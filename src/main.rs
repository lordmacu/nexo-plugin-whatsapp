//! Subprocess entrypoint for `nexo-plugin-whatsapp` (Phase 81.19.a).
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
//! `proyecto/src/main.rs::seed_whatsapp_subprocess_env` (deferred
//! `81.18.b`):
//!   * `NEXO_PLUGIN_WHATSAPP_INSTANCE`         (optional)
//!   * `NEXO_PLUGIN_WHATSAPP_SESSION_DIR`
//!   * `NEXO_PLUGIN_WHATSAPP_MEDIA_DIR`
//!   * `NEXO_PLUGIN_WHATSAPP_BRIDGE_TIMEOUT_MS` (default 30000)
//!   * `NEXO_PLUGIN_WHATSAPP_ALLOWLIST`         (JSON array, optional)
//!   * `NEXO_PLUGIN_WHATSAPP_TRANSCRIBE_ENABLED` (default false)
//!   * `NEXO_PLUGIN_WHATSAPP_WHISPER_TIMEOUT_MS` (default 60000)
//!   * `NEXO_BROKER_URL`

use std::sync::Arc;

use nexo_broker::AnyBroker;
use nexo_core::agent::plugin::Plugin;
use nexo_microapp_sdk::plugin::{PluginAdapter, ToolInvocation, ToolInvocationError};
use nexo_plugin_whatsapp::{
    dispatch_whatsapp_tool, whatsapp_config_from_env, whatsapp_tool_defs, WhatsappPlugin,
};
use once_cell::sync::Lazy;
use tokio::sync::OnceCell;

const MANIFEST: &str = include_str!("../nexo-plugin.toml");

/// Process-wide [`WhatsappPlugin`]. Boot is gated behind the first
/// `tool.invoke` so the JSON-RPC `initialize` handshake can complete
/// even when the broker is unreachable at startup. Daemon supervisor
/// retries broker / Signal Protocol outages on its own cadence.
static PLUGIN: Lazy<OnceCell<Arc<WhatsappPlugin>>> = Lazy::new(OnceCell::new);

async fn shared_plugin() -> Result<Arc<WhatsappPlugin>, ToolInvocationError> {
    PLUGIN
        .get_or_try_init(|| async {
            let cfg = whatsapp_config_from_env()
                .map_err(|e| ToolInvocationError::ArgumentInvalid(format!("env config: {e}")))?;

            let broker_url = std::env::var("NEXO_BROKER_URL").map_err(|_| {
                ToolInvocationError::Unavailable(
                    "NEXO_BROKER_URL not set — daemon must seed it before tool.invoke".into(),
                )
            })?;

            // Build a `BrokerInner` from the seeded URL. Auth /
            // persistence / limits / fallback all default — the
            // daemon already chose those for the parent process and
            // the subprocess just needs the connection URL to reach
            // the same NATS server.
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

            let broker = AnyBroker::from_config(&broker_inner).await.map_err(|e| {
                ToolInvocationError::Unavailable(format!("broker connect failed: {e}"))
            })?;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    // a known wart tracked under `81.19.a.tls-rustls`.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let adapter = PluginAdapter::new(MANIFEST)?
        .declare_tools(whatsapp_tool_defs())
        .on_tool(|invocation: ToolInvocation| async move {
            let plugin = shared_plugin().await?;
            dispatch_whatsapp_tool(plugin.as_ref(), invocation).await
        });

    adapter.run_stdio().await?;
    Ok(())
}
