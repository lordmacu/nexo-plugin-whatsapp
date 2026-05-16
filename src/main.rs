//! Subprocess entrypoint for `nexo-plugin-whatsapp`.
//!
//! Wires:
//!   - [`PluginAdapter`] — child-side JSON-RPC dispatch loop.
//!   - [`whatsapp_tool_defs`] — 4 `whatsapp_*` tool defs advertised
//!     in the initialize reply.
//!   - [`dispatch_whatsapp_tool`] — per-tool routing through the
//!     plugin's broker-publish path.
//!   - one [`WhatsappPlugin`] per process, lazy-booted on the first
//!     `tool.invoke` from the daemon-supplied env vars.
//!
//! Configuration flows from the daemon via env vars set by
//! `proyecto/src/main.rs::seed_whatsapp_subprocess_env`:
//!   * `NEXO_PLUGIN_WHATSAPP_INSTANCE`         (optional)
//!   * `NEXO_PLUGIN_WHATSAPP_SESSION_DIR`
//!   * `NEXO_PLUGIN_WHATSAPP_MEDIA_DIR`
//!   * `NEXO_PLUGIN_WHATSAPP_BRIDGE_TIMEOUT_MS` (default 30000)
//!   * `NEXO_PLUGIN_WHATSAPP_ALLOWLIST`         (JSON array, optional)
//!   * `NEXO_PLUGIN_WHATSAPP_TRANSCRIBE_ENABLED` (default false)
//!   * `NEXO_PLUGIN_WHATSAPP_WHISPER_TIMEOUT_MS` (default 60000)
//!   * `NEXO_BROKER_KIND`  (`nats` or `stdio_bridge`; defaults to
//!                          `nats` for backwards compat with daemons
//!                          that pre-date the env stamp)
//!   * `NEXO_BROKER_URL`  (required when KIND=nats; ignored
//!                          when KIND=stdio_bridge — the transport
//!                          is the parent process's stdin/stdout)

use std::sync::Arc;

use nexo_broker::{AnyBroker, StdioBridgeBroker};
use nexo_core::agent::plugin::Plugin;
use nexo_microapp_sdk::plugin::{PluginAdapter, ToolInvocation, ToolInvocationError};
use nexo_plugin_whatsapp::{
    dispatch_whatsapp_tool, whatsapp_config_from_env, whatsapp_tool_defs, WhatsappPlugin,
};
use once_cell::sync::Lazy;
use tokio::sync::OnceCell;

const MANIFEST: &str = include_str!("../nexo-plugin.toml");

/// Populated in `main()` when the daemon stamps
/// `NEXO_BROKER_KIND=stdio_bridge`. The bridge holds the outbound
/// mpsc Sender wired into the PluginAdapter's drain task and the
/// inbound subscriber fanout. `shared_plugin()` clones from this
/// OnceCell when building the broker for the stdio_bridge path.
static BRIDGE: Lazy<OnceCell<Arc<StdioBridgeBroker>>> = Lazy::new(OnceCell::new);

/// Process-wide [`WhatsappPlugin`]. Boot is gated behind the first
/// `tool.invoke` so the JSON-RPC `initialize` handshake can complete
/// even when the broker is unreachable at startup. Daemon supervisor
/// retries broker / Signal Protocol outages on its own cadence.
static PLUGIN: Lazy<OnceCell<Arc<WhatsappPlugin>>> = Lazy::new(OnceCell::new);

/// Construct the broker the plugin uses for
/// publish/subscribe. Branches on `NEXO_BROKER_KIND`:
///
/// - `stdio_bridge` (or empty default → fall back to nats for
///   backwards compatibility): use the `StdioBridgeBroker` placed
///   into [`BRIDGE`] by `main()`. Pre-92 daemons that don't
///   stamp `NEXO_BROKER_KIND` keep working through the nats
///   fallback path below.
/// - `nats` (or unset): connect to the seeded `NEXO_BROKER_URL`.
async fn build_broker() -> Result<AnyBroker, ToolInvocationError> {
    let kind = std::env::var("NEXO_BROKER_KIND").unwrap_or_else(|_| "nats".to_string());
    if kind == "stdio_bridge" {
        let bridge = BRIDGE.get().ok_or_else(|| {
            ToolInvocationError::Unavailable(
                "stdio_bridge mode: BRIDGE not initialized — main() must call \
                 PluginAdapter::with_stdio_bridge_broker before tool.invoke"
                    .into(),
            )
        })?;
        return Ok(AnyBroker::stdio_bridge((**bridge).clone()));
    }
    // Default + explicit `nats` path: connect to the broker URL
    // the daemon seeded. Pre-92 daemons that don't set
    // `NEXO_BROKER_KIND` land here too (legacy compat).
    let broker_url = std::env::var("NEXO_BROKER_URL").map_err(|_| {
        ToolInvocationError::Unavailable(
            "NEXO_BROKER_URL not set — daemon must seed it before tool.invoke".into(),
        )
    })?;
    let broker_inner = nexo_config::types::broker::BrokerInner {
        kind: if broker_url.starts_with("nats://") {
            nexo_config::types::broker::BrokerKind::Nats
        } else {
            nexo_config::types::broker::BrokerKind::Local
        },
        url: broker_url,
        auth: nexo_config::types::broker::BrokerAuthConfig::default(),
        persistence: nexo_config::types::broker::BrokerPersistenceConfig::default(),
        limits: nexo_config::types::broker::BrokerLimitsConfig::default(),
        fallback: nexo_config::types::broker::BrokerFallbackConfig::default(),
    };
    AnyBroker::from_config(&broker_inner)
        .await
        .map_err(|e| ToolInvocationError::Unavailable(format!("broker connect failed: {e}")))
}

async fn shared_plugin() -> Result<Arc<WhatsappPlugin>, ToolInvocationError> {
    PLUGIN
        .get_or_try_init(|| async {
            // Phase 93.4.b — prefer the `plugin.configure`-delivered
            // slice (Phase 93.2) when present; legacy env-var path
            // stays as fallback during the 0.2.x deprecation window.
            let cfg = {
                let guard = nexo_plugin_whatsapp::configured_state().read().await;
                if let Some(vec) = guard.as_ref() {
                    vec.first().cloned().ok_or_else(|| {
                        ToolInvocationError::ArgumentInvalid(
                            "configured array delivered empty Vec".into(),
                        )
                    })?
                } else {
                    drop(guard);
                    whatsapp_config_from_env().map_err(|e| {
                        ToolInvocationError::ArgumentInvalid(format!("env config: {e}"))
                    })?
                }
            };

            let broker = build_broker().await?;

            let plugin = Arc::new(WhatsappPlugin::new(cfg));

            // `start` walks `cfg.session_dir` for Signal credentials,
            // bootstraps wa-agent's `Client::new_in_dir`, subscribes
            // the outbound dispatcher, spawns the inbound bridge, and
            // either restores an existing pairing or queues a fresh
            // QR. A 401 / corrupt-creds / network outage here surfaces
            // as Unavailable so the daemon supervisor handles retry.
            // Subsequent `tool.invoke` calls find the cached plugin.
            plugin.start(broker).await.map_err(|e| {
                ToolInvocationError::Unavailable(format!("whatsapp plugin start failed: {e}"))
            })?;

            tracing::info!(
                target = "nexo_plugin_whatsapp",
                instance = plugin.config().instance.as_deref().unwrap_or(""),
                "whatsapp subprocess plugin ready"
            );
            Ok::<_, ToolInvocationError>(plugin)
        })
        .await
        .cloned()
}

/// Scan `std::env::args()` for the `--pair-once <session_dir>`
/// CLI shape the setup wizard spawns. Returns `Some(session_dir)`
/// when both `--pair-once` and a following positional argument
/// are present, `None` otherwise. Tolerates the flag in any
/// position so future arg-handling layers (e.g. `--print-manifest`
/// which fires first) don't clobber the parse.
fn parse_pair_once_arg() -> Option<String> {
    let mut iter = std::env::args().skip(1);
    while let Some(a) = iter.next() {
        if a == "--pair-once" {
            return iter.next();
        }
    }
    None
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Stage 8 cargo-install ergonomics. When the daemon's binary-
    // mode discovery walker probes us with
    // `nexo-plugin-whatsapp --print-manifest` we emit the bundled
    // TOML to stdout and exit 0 BEFORE tracing init / broker
    // wiring — the walker needs only the manifest bytes.
    nexo_microapp_sdk::plugin::print_manifest_if_requested(MANIFEST);

    // Phase 81.20.x Bucket C2 BC.4 — `--pair-once <session_dir>`
    // subcommand. The nexo setup wizard spawns us in this mode
    // for interactive WhatsApp pairing instead of calling
    // `nexo_plugin_whatsapp::session::pair_once` in-process from
    // the setup crate. Eliminates setup's `nexo-plugin-whatsapp`
    // Cargo dep so the framework setup tooling stays
    // plugin-agnostic. We run wa-agent's QR flow, print the QR
    // ASCII to stdout, exit when the user scans + Signal session
    // persists.
    if let Some(session_dir) = parse_pair_once_arg() {
        let session_path = std::path::PathBuf::from(session_dir);
        return nexo_plugin_whatsapp::session::pair_once(&session_path).await;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // rustls 0.23 requires an explicit process-wide CryptoProvider
    // before `ClientConfig::builder()` can return successfully.
    // Same dance as the proyecto daemon (see proyecto/src/main.rs).
    // wa-agent itself uses native-tls (OpenSSL); the dual stack is
    // a known wart.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let adapter = PluginAdapter::new(MANIFEST)?
        .declare_tools(whatsapp_tool_defs())
        // Phase 93.4.b — receive the operator YAML slice via the
        // host's `plugin.configure` JSON-RPC (Phase 93.2). Sequence
        // shape per manifest `[plugin.config_schema] shape = "array"`.
        .on_configure(|value: serde_yaml::Value| async move {
            let parsed: Vec<nexo_plugin_whatsapp::WhatsappPluginConfig> =
                serde_yaml::from_value(value)
                    .map_err(|e| format!("invalid whatsapp config: {e}"))?;
            *nexo_plugin_whatsapp::configured_state().write().await = Some(parsed);
            Ok(())
        })
        // Phase 93.8.b — credential-store handlers. Daemon-side
        // `RemoteCredentialStore` (Phase 93.8.a-daemon) round-trips
        // each `GenericCredentialStore` method here.
        .on_credentials_list(|| async move {
            let guard = nexo_plugin_whatsapp::configured_state().read().await;
            let accounts: Vec<String> = guard
                .as_ref()
                .map(|v| v.iter().filter_map(|c| c.instance.clone()).collect())
                .unwrap_or_default();
            Ok(nexo_microapp_sdk::plugin::CredentialsListReply {
                accounts,
                warnings: Vec::new(),
            })
        })
        .on_credentials_issue(|account_id: String, agent_id: String| async move {
            // Allow-list check against the matching account's
            // `allow_agents`. Empty list ⇒ accept any.
            let guard = nexo_plugin_whatsapp::configured_state().read().await;
            let Some(cfgs) = guard.as_ref() else {
                return Err("not_found".to_string());
            };
            let cfg = cfgs
                .iter()
                .find(|c| c.instance.as_deref() == Some(account_id.as_str()));
            match cfg {
                None => Err("not_found".to_string()),
                Some(c) if c.allow_agents.is_empty() || c.allow_agents.contains(&agent_id) => {
                    Ok(())
                }
                Some(_) => Err("not_permitted".to_string()),
            }
        })
        .on_credentials_resolve_bytes(
            |account_id: String, _agent_id: String, _fingerprint: String| async move {
                let guard = nexo_plugin_whatsapp::configured_state().read().await;
                let Some(cfgs) = guard.as_ref() else {
                    return Err("not_found".to_string());
                };
                let cfg = cfgs
                    .iter()
                    .find(|c| c.instance.as_deref() == Some(account_id.as_str()))
                    .ok_or_else(|| "not_found".to_string())?;
                serde_json::to_vec(cfg).map_err(|e| format!("serialize failed: {e}"))
            },
        )
        .on_credentials_reload(|| async move {
            // WhatsApp's Signal Protocol session state isn't
            // re-readable on a whim. Operator YAML changes re-flow
            // via `plugin.configure`. No-op ack.
            Ok(())
        })
        .on_tool(|invocation: ToolInvocation| async move {
            let plugin = shared_plugin().await?;
            dispatch_whatsapp_tool(plugin.as_ref(), invocation).await
        });

    // When the daemon stamps
    // `NEXO_BROKER_KIND=stdio_bridge`, wire the adapter's outbound
    // drain + on_broker_event handler to a fresh StdioBridgeBroker
    // and stash it in the `BRIDGE` OnceCell so `build_broker()`
    // hands it out to `shared_plugin()` instead of constructing a
    // NATS connection. The bridge piggybacks on the adapter's
    // stdout writer; net: zero network broker dependency.
    let adapter = if std::env::var("NEXO_BROKER_KIND").as_deref() == Ok("stdio_bridge") {
        let (adapter, bridge) = adapter.with_stdio_bridge_broker();
        BRIDGE
            .set(bridge)
            .map_err(|_| anyhow::anyhow!("BRIDGE already initialized (this should not happen)"))?;
        tracing::info!(
            target = "nexo_plugin_whatsapp",
            "stdio_bridge broker wired (daemon broker = Local)"
        );
        adapter
    } else {
        adapter
    };

    // Phase 81.33.b.real v0.4 — auto-discovery broker subscriber
    // loop. Spawned unconditionally for both stdio_bridge and
    // NATS modes so daemon-published requests on
    // `plugin.whatsapp.*` reach handlers in
    // `auto_discovery::*`. Lib-linked daemons (feature on) skip
    // main.rs entirely, so spawning here is safe.
    match auto_discovery_broker().await {
        Ok(broker) => spawn_auto_discovery_subscribers(broker),
        Err(e) => tracing::warn!(
            target = "nexo_plugin_whatsapp",
            error = %e,
            "auto-discovery broker unavailable; subscribers not spawned (tool.invoke path unaffected)"
        ),
    }

    // Eagerly boot the plugin so the inbound bridge connects to
    // WhatsApp BEFORE the first `tool.invoke`. Without this the
    // subprocess sits idle on stdio after handshake — no Signal
    // session, no broker subscription, no inbound deliveries to
    // the daemon. The lazy-OnceCell path inside `shared_plugin`
    // still covers `tool.invoke` re-entries (no duplicate boot).
    // Boot failures here are logged but NOT fatal: the plugin
    // host's supervisor expects `run_stdio` to keep serving the
    // initialize handshake; a transient outage at startup should
    // not crash the process and trigger a respawn loop.
    if let Err(e) = shared_plugin().await {
        tracing::warn!(
            target = "nexo_plugin_whatsapp",
            error = %e,
            "eager start failed; falling back to lazy start on first tool.invoke"
        );
    }

    adapter.run_stdio().await?;
    Ok(())
}

/// Construct the broker handle the auto-discovery subscriber loop
/// reads from. Mirrors `build_broker` but returns `anyhow` so
/// startup wiring can log + skip cleanly instead of failing the
/// whole process — a plugin without subscribers still answers
/// `tool.invoke` via the JSON-RPC channel.
async fn auto_discovery_broker() -> anyhow::Result<AnyBroker> {
    let kind = std::env::var("NEXO_BROKER_KIND").unwrap_or_else(|_| "nats".to_string());
    if kind == "stdio_bridge" {
        let bridge = BRIDGE
            .get()
            .ok_or_else(|| anyhow::anyhow!("BRIDGE not initialized"))?;
        return Ok(AnyBroker::stdio_bridge((**bridge).clone()));
    }
    let url = std::env::var("NEXO_BROKER_URL")
        .map_err(|_| anyhow::anyhow!("NEXO_BROKER_URL not set"))?;
    let inner = nexo_config::types::broker::BrokerInner {
        kind: if url.starts_with("nats://") {
            nexo_config::types::broker::BrokerKind::Nats
        } else {
            nexo_config::types::broker::BrokerKind::Local
        },
        url,
        auth: nexo_config::types::broker::BrokerAuthConfig::default(),
        persistence: nexo_config::types::broker::BrokerPersistenceConfig::default(),
        limits: nexo_config::types::broker::BrokerLimitsConfig::default(),
        fallback: nexo_config::types::broker::BrokerFallbackConfig::default(),
    };
    AnyBroker::from_config(&inner)
        .await
        .map_err(|e| anyhow::anyhow!("broker connect failed: {e}"))
}

/// Phase 81.33.b.real v0.4 — auto-discovery broker subscriber
/// loop. Spawns one tokio task per request-reply topic family.
/// Each task subscribes, parses `Message` from each inbound
/// `Event.payload`, dispatches to the matching async handler,
/// and publishes the reply back to `msg.reply_to`.
fn spawn_auto_discovery_subscribers(broker: AnyBroker) {
    use nexo_plugin_whatsapp::auto_discovery as ad;

    spawn_one(broker.clone(), "plugin.whatsapp.pairing.normalize_sender", |_b, p| async move {
        ad::pairing_normalize_sender(&p)
    });
    spawn_one(broker.clone(), "plugin.whatsapp.pairing.send_reply", |b, p| async move {
        ad::pairing_send_reply(&b, &p).await
    });
    spawn_one(broker.clone(), "plugin.whatsapp.pairing.send_qr_image", |b, p| async move {
        ad::pairing_send_qr_image(&b, &p).await
    });
    spawn_one(broker.clone(), "plugin.whatsapp.http.request", |_b, p| async move {
        ad::http_request(&p).await
    });
    spawn_one(broker.clone(), "plugin.whatsapp.metrics.scrape", |_b, p| async move {
        ad::metrics_scrape(&p).await
    });
    spawn_one(broker, "plugin.whatsapp.admin.>", |b, p| async move {
        ad::admin_handle(&b, &p).await
    });
}

fn spawn_one<F, Fut>(broker: AnyBroker, topic: &'static str, handler: F)
where
    F: Fn(AnyBroker, serde_json::Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = serde_json::Value> + Send + 'static,
{
    use nexo_broker::{BrokerHandle, Event, Message};
    tokio::spawn(async move {
        let mut sub = match broker.subscribe(topic).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    target = "whatsapp.auto_discovery",
                    topic,
                    error = %e,
                    "subscribe failed; topic will not receive requests"
                );
                return;
            }
        };
        tracing::info!(target = "whatsapp.auto_discovery", topic, "subscriber up");
        while let Some(event) = sub.next().await {
            let Ok(msg) = serde_json::from_value::<Message>(event.payload) else {
                continue;
            };
            let Some(reply_to) = msg.reply_to.clone() else {
                continue;
            };
            let reply_payload = handler(broker.clone(), msg.payload.clone()).await;
            let reply_msg = Message::new(reply_to.clone(), reply_payload);
            let reply_event = Event::new(
                reply_to.clone(),
                "whatsapp",
                match serde_json::to_value(&reply_msg) {
                    Ok(v) => v,
                    Err(_) => continue,
                },
            );
            if let Err(e) = broker.publish(&reply_to, reply_event).await {
                tracing::warn!(
                    target = "whatsapp.auto_discovery",
                    topic,
                    reply_to = %reply_to,
                    error = %e,
                    "failed to publish reply"
                );
            }
        }
        tracing::debug!(target = "whatsapp.auto_discovery", topic, "subscriber stream ended");
    });
}
