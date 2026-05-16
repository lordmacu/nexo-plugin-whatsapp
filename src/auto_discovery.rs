//! Phase 81.33.b.real Stages 1+2+4+5 — auto-discovery broker
//! handlers (v0.4 — full real wiring).
//!
//! Pure or async functions that take a JSON request payload + an
//! optional broker handle and return a JSON response payload.
//! Wired in `src/main.rs` via the broker subscription loop:
//! incoming `broker.event` →
//! `serde_json::from_value::<Message>(payload)` → invoke handler
//! → publish reply to `msg.reply_to`.
//!
//! Contract docs in the daemon repo:
//! - Pairing adapter — `proyecto/docs/src/plugins/manifest-pairing-adapter.md`
//! - HTTP routes     — `proyecto/docs/src/plugins/manifest-http.md`
//! - Admin RPC       — `proyecto/docs/src/plugins/manifest-admin.md`
//! - Metrics scrape  — `proyecto/docs/src/plugins/manifest-metrics.md`

use base64::Engine;
use nexo_broker::{AnyBroker, BrokerHandle, Event};
use serde_json::{json, Value};

use crate::configured_state;

// ── Stage 1 — pairing adapter ──────────────────────────────────

/// Canonicalise a raw WhatsApp sender JID into the E.164 form
/// the rest of the system keys allowlists by. Mirrors
/// `WhatsappPairingAdapter::normalize_sender`.
///
/// Strips the `@c.us` / `@s.whatsapp.net` suffix, prepends `+`
/// when missing. Empty digits → reject.
pub fn pairing_normalize_sender(request: &Value) -> Value {
    let raw = request.get("raw").and_then(|v| v.as_str()).unwrap_or("");
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return json!({ "normalized": null });
    }
    let stripped = trimmed
        .strip_suffix("@c.us")
        .or_else(|| trimmed.strip_suffix("@s.whatsapp.net"))
        .unwrap_or(trimmed);
    if stripped.is_empty() {
        return json!({ "normalized": null });
    }
    let normalized = if stripped.starts_with('+') {
        stripped.to_string()
    } else {
        format!("+{stripped}")
    };
    json!({ "normalized": normalized })
}

fn outbound_topic(account: &str) -> String {
    if account.is_empty() {
        "plugin.outbound.whatsapp".to_string()
    } else {
        format!("plugin.outbound.whatsapp.{account}")
    }
}

/// Deliver a plain-text pairing reply by publishing to the
/// plugin's outbound topic. Same payload shape
/// `WhatsappPairingAdapter::send_reply` emits — the outbound
/// dispatcher in `lifecycle.rs` consumes the event + issues the
/// `wa-agent` `send_text` call.
pub async fn pairing_send_reply(broker: &AnyBroker, request: &Value) -> Value {
    let account = request
        .get("account")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let to = request.get("to").and_then(|v| v.as_str()).unwrap_or("");
    let text = request.get("text").and_then(|v| v.as_str()).unwrap_or("");
    if to.is_empty() || text.is_empty() {
        return json!({ "ok": false, "error": "to and text required" });
    }
    let topic = outbound_topic(account);
    let payload = json!({
        "kind": "text",
        "to": to,
        "text": text,
    });
    let evt = Event::new(&topic, "core.pairing", payload);
    match broker.publish(&topic, evt).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": format!("publish failed: {e}") }),
    }
}

/// Send a QR PNG via the outbound dispatcher. Validates base64
/// and publishes a `kind="photo"` event on the outbound topic.
pub async fn pairing_send_qr_image(broker: &AnyBroker, request: &Value) -> Value {
    let account = request
        .get("account")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let to = request.get("to").and_then(|v| v.as_str()).unwrap_or("");
    let png_b64 = request
        .get("png_base64")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let caption = request
        .get("caption")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if to.is_empty() || png_b64.is_empty() {
        return json!({ "ok": false, "error": "to and png_base64 required" });
    }
    if base64::engine::general_purpose::STANDARD
        .decode(png_b64.as_bytes())
        .map(|b| b.is_empty())
        .unwrap_or(true)
    {
        return json!({ "ok": false, "error": "invalid or empty base64" });
    }
    let topic = outbound_topic(account);
    let payload = json!({
        "kind": "photo",
        "to": to,
        "png_base64": png_b64,
        "caption": caption,
    });
    let evt = Event::new(&topic, "core.pairing", payload);
    match broker.publish(&topic, evt).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": format!("publish failed: {e}") }),
    }
}

// ── Stage 2 — HTTP routes ──────────────────────────────────────

/// Handle an HTTP request the daemon proxied under `/whatsapp/*`.
///
/// Routes:
/// - `GET /whatsapp/health` — plain-text health probe.
/// - `GET /whatsapp/status` — JSON snapshot of plugin + configured
///   instances.
/// - anything else → 404 (the legacy `/whatsapp/pair{,/qr,/status}`
///   QR-pairing routes remain on the daemon's lib-linked path
///   during the migration window — Stage 7 follow-up moves them
///   here once the broker contract exposes `SharedPairingState`).
pub async fn http_request(request: &Value) -> Value {
    let path = request.get("path").and_then(|v| v.as_str()).unwrap_or("/");
    let method = request
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET");
    match (method, path) {
        ("GET", "/whatsapp/health") => respond(
            200,
            "text/plain; charset=utf-8",
            b"whatsapp plugin ok\n",
        ),
        ("GET", "/whatsapp/status") => {
            let instances = configured_instances().await;
            let body = json!({
                "status": "ok",
                "plugin": "whatsapp",
                "version": env!("CARGO_PKG_VERSION"),
                "configured_instances": instances,
            });
            respond(
                200,
                "application/json; charset=utf-8",
                body.to_string().as_bytes(),
            )
        }
        _ => respond(
            404,
            "application/json; charset=utf-8",
            br#"{"error":"not found"}"#,
        ),
    }
}

fn respond(status: u16, content_type: &str, body: &[u8]) -> Value {
    json!({
        "status": status,
        "headers": [["Content-Type", content_type]],
        "body_base64": base64::engine::general_purpose::STANDARD.encode(body),
    })
}

// ── Stage 4 — admin RPC ────────────────────────────────────────

/// Handle a daemon-forwarded admin RPC.
///
/// Methods:
/// - `nexo/admin/whatsapp/bot_info` — plugin metadata + configured instance count.
/// - `nexo/admin/whatsapp/list_instances` — declared instance ids.
/// - `nexo/admin/whatsapp/pairing/start` — v0.4.4: spawn the
///   wa-agent QR pump for `params.challenge_id` + `params.instance`.
///   QR rotations and terminal state arrive on
///   `plugin.inbound.whatsapp.<inst>.pairing.{qr,state}`.
/// - `nexo/admin/whatsapp/pairing/cancel` — v0.4.4: abort an
///   in-flight pump by `params.challenge_id`. Idempotent.
pub async fn admin_handle(broker: &AnyBroker, request: &Value) -> Value {
    let method = request
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match method {
        "nexo/admin/whatsapp/bot_info" => {
            let instances = configured_instances().await;
            json!({
                "ok": true,
                "result": {
                    "plugin": "whatsapp",
                    "version": env!("CARGO_PKG_VERSION"),
                    "configured_instances": instances,
                },
            })
        }
        "nexo/admin/whatsapp/list_instances" => {
            let instances = configured_instances().await;
            json!({ "ok": true, "result": { "instances": instances } })
        }
        "nexo/admin/whatsapp/pairing/start" => {
            crate::pairing_admin::pairing_start(broker.clone(), request).await
        }
        "nexo/admin/whatsapp/pairing/cancel" => {
            crate::pairing_admin::pairing_cancel(broker.clone(), request).await
        }
        other => json!({
            "ok": false,
            "error": format!("unknown admin method: {other}"),
        }),
    }
}

// ── Stage 5 — metrics scrape ───────────────────────────────────

/// Emit Prometheus text for the daemon's `/metrics` aggregator.
/// Series prefixed with `whatsapp_` to avoid collisions.
pub async fn metrics_scrape(_request: &Value) -> Value {
    let instance_count = configured_instances().await.len();
    let version = env!("CARGO_PKG_VERSION");
    let text = format!(
        "# HELP whatsapp_plugin_ready Whether the whatsapp plugin is up.\n\
         # TYPE whatsapp_plugin_ready gauge\n\
         whatsapp_plugin_ready 1\n\
         # HELP whatsapp_plugin_version_info Plugin version label.\n\
         # TYPE whatsapp_plugin_version_info gauge\n\
         whatsapp_plugin_version_info{{version=\"{version}\"}} 1\n\
         # HELP whatsapp_plugin_instances_configured Configured instance count.\n\
         # TYPE whatsapp_plugin_instances_configured gauge\n\
         whatsapp_plugin_instances_configured {instance_count}\n",
    );
    json!({ "text": text })
}

// ── helpers ────────────────────────────────────────────────────

async fn configured_instances() -> Vec<String> {
    let guard = configured_state().read().await;
    guard
        .as_ref()
        .map(|cfgs| {
            cfgs.iter()
                .filter_map(|c| c.instance.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexo_broker::{AnyBroker, BrokerHandle};

    #[test]
    fn pairing_normalize_strips_c_us_and_adds_plus() {
        let r = pairing_normalize_sender(&json!({ "raw": "573001112222@c.us" }));
        assert_eq!(r["normalized"].as_str(), Some("+573001112222"));
    }

    #[test]
    fn pairing_normalize_strips_s_whatsapp_net() {
        let r = pairing_normalize_sender(&json!({ "raw": "573001112222@s.whatsapp.net" }));
        assert_eq!(r["normalized"].as_str(), Some("+573001112222"));
    }

    #[test]
    fn pairing_normalize_keeps_existing_plus() {
        let r = pairing_normalize_sender(&json!({ "raw": "+573001112222@c.us" }));
        assert_eq!(r["normalized"].as_str(), Some("+573001112222"));
    }

    #[test]
    fn pairing_normalize_handles_bare_digits() {
        let r = pairing_normalize_sender(&json!({ "raw": "573001112222" }));
        assert_eq!(r["normalized"].as_str(), Some("+573001112222"));
    }

    #[test]
    fn pairing_normalize_rejects_empty_after_strip() {
        let r = pairing_normalize_sender(&json!({ "raw": "@c.us" }));
        assert!(r["normalized"].is_null());
    }

    #[test]
    fn pairing_normalize_rejects_empty_input() {
        let r = pairing_normalize_sender(&json!({ "raw": "" }));
        assert!(r["normalized"].is_null());
        let r = pairing_normalize_sender(&json!({ "raw": "   " }));
        assert!(r["normalized"].is_null());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pairing_send_reply_publishes_to_outbound_topic_when_account_empty() {
        let broker = AnyBroker::local();
        let mut sub = broker.subscribe("plugin.outbound.whatsapp").await.unwrap();
        let r = pairing_send_reply(
            &broker,
            &json!({ "account": "", "to": "+573001112222", "text": "hello" }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(true));
        let evt = sub.next().await.expect("event published");
        assert_eq!(evt.payload["kind"].as_str(), Some("text"));
        assert_eq!(evt.payload["to"].as_str(), Some("+573001112222"));
        assert_eq!(evt.payload["text"].as_str(), Some("hello"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pairing_send_reply_uses_per_instance_topic_when_account_set() {
        let broker = AnyBroker::local();
        let mut sub = broker
            .subscribe("plugin.outbound.whatsapp.primary")
            .await
            .unwrap();
        let r = pairing_send_reply(
            &broker,
            &json!({ "account": "primary", "to": "+1", "text": "x" }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(true));
        sub.next().await.expect("event published");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pairing_send_reply_rejects_missing_fields() {
        let broker = AnyBroker::local();
        let r = pairing_send_reply(
            &broker,
            &json!({ "account": "default", "to": "", "text": "x" }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(false));
        let r = pairing_send_reply(
            &broker,
            &json!({ "account": "default", "to": "+1", "text": "" }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(false));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pairing_send_qr_image_publishes_photo_event() {
        let broker = AnyBroker::local();
        let mut sub = broker.subscribe("plugin.outbound.whatsapp").await.unwrap();
        let png_b64 = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\n");
        let r = pairing_send_qr_image(
            &broker,
            &json!({
                "account": "",
                "to": "+1",
                "png_base64": png_b64,
                "caption": "scan to pair",
            }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(true));
        let evt = sub.next().await.expect("event published");
        assert_eq!(evt.payload["kind"].as_str(), Some("photo"));
        assert_eq!(evt.payload["caption"].as_str(), Some("scan to pair"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pairing_send_qr_image_rejects_invalid_base64() {
        let broker = AnyBroker::local();
        let r = pairing_send_qr_image(
            &broker,
            &json!({ "account": "", "to": "+1", "png_base64": "!!nope!!" }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(false));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pairing_send_qr_image_rejects_empty_base64() {
        let broker = AnyBroker::local();
        let r = pairing_send_qr_image(
            &broker,
            &json!({ "account": "", "to": "+1", "png_base64": "" }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(false));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_get_health_serves_200() {
        let r = http_request(&json!({ "method": "GET", "path": "/whatsapp/health" })).await;
        assert_eq!(r["status"].as_u64(), Some(200));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_get_status_returns_plugin_metadata() {
        let r = http_request(&json!({ "method": "GET", "path": "/whatsapp/status" })).await;
        assert_eq!(r["status"].as_u64(), Some(200));
        let body_b64 = r["body_base64"].as_str().unwrap();
        let body = base64::engine::general_purpose::STANDARD
            .decode(body_b64)
            .unwrap();
        let body_str = String::from_utf8(body).unwrap();
        assert!(body_str.contains("\"plugin\":\"whatsapp\""));
        assert!(body_str.contains("\"version\""));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_unknown_returns_404() {
        let r = http_request(&json!({ "method": "GET", "path": "/whatsapp/missing" })).await;
        assert_eq!(r["status"].as_u64(), Some(404));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn admin_bot_info_returns_plugin_metadata() {
        let broker = AnyBroker::local();
        let r = admin_handle(
            &broker,
            &json!({
                "method": "nexo/admin/whatsapp/bot_info",
                "params": {},
            }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(true));
        assert_eq!(r["result"]["plugin"].as_str(), Some("whatsapp"));
        assert!(r["result"]["version"].is_string());
        assert!(r["result"]["configured_instances"].is_array());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn admin_list_instances_returns_array() {
        let broker = AnyBroker::local();
        let r = admin_handle(
            &broker,
            &json!({
                "method": "nexo/admin/whatsapp/list_instances",
                "params": {},
            }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(true));
        assert!(r["result"]["instances"].is_array());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn admin_unknown_method_returns_err() {
        let broker = AnyBroker::local();
        let r = admin_handle(
            &broker,
            &json!({
                "method": "nexo/admin/whatsapp/nonexistent",
                "params": {},
            }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(false));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn metrics_scrape_returns_whatsapp_namespaced_metrics() {
        let r = metrics_scrape(&json!({})).await;
        let text = r["text"].as_str().expect("text");
        assert!(text.contains("whatsapp_plugin_ready 1"));
        assert!(text.contains("whatsapp_plugin_version_info"));
        assert!(text.contains("whatsapp_plugin_instances_configured"));
    }
}
