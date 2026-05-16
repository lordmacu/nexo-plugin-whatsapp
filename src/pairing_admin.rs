//! v0.4.4 — admin-RPC-driven pairing pump.
//!
//! Plugin v0.4.4 (Phase 81.20.x Stage 7 Phase 2) replaces the
//! daemon-side `WhatsappPairingTrigger` import with two admin-RPC
//! methods served entirely inside this subprocess:
//!
//! - `nexo/admin/whatsapp/pairing/start` — spawns the wa-agent
//!   QR pump for the requested challenge, publishes QR rotations
//!   and terminal state on `plugin.inbound.whatsapp.<inst>.pairing.{qr,state}`.
//! - `nexo/admin/whatsapp/pairing/cancel` — aborts an in-flight
//!   pump by challenge id.
//!
//! Daemon's [`BrokerPairingTrigger`] (in `nexo-pairing`) forwards
//! `pairing/start` here via the [`plugin.admin`] router, then
//! consumes the inbound topics to update its shared pairing
//! store + notifier. The daemon no longer links this crate.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

use dashmap::DashMap;
use nexo_broker::{AnyBroker, BrokerHandle, Event};
use serde_json::{json, Value};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::configured_state;
use crate::session::pair_with_callback;

/// In-flight pump registry. Keyed by `challenge_id`. Survives the
/// duration of one pairing handshake; on completion / cancel /
/// failure the entry is removed by the pump task itself.
struct PumpHandle {
    cancel: CancellationToken,
    /// JoinHandle for the spawned pump task. Held so a duplicate
    /// `pairing/start` for the same challenge can cancel + replace
    /// cleanly.
    _task: JoinHandle<()>,
}

fn pumps() -> &'static Arc<DashMap<Uuid, PumpHandle>> {
    static PUMPS: OnceLock<Arc<DashMap<Uuid, PumpHandle>>> = OnceLock::new();
    PUMPS.get_or_init(|| Arc::new(DashMap::new()))
}

/// Resolve `instance` → on-disk session dir using the
/// `plugin.configure`-delivered config slice. Falls back to the
/// first declared config when the caller passes `None` (legacy
/// single-account mode).
async fn resolve_session_dir(instance: Option<&str>) -> Option<(String, PathBuf)> {
    let guard = configured_state().read().await;
    let cfgs = guard.as_ref()?;
    let target = instance.unwrap_or("");
    let matched = cfgs.iter().find(|c| {
        c.instance.as_deref().unwrap_or("") == target
    });
    let chosen = matched.or_else(|| cfgs.first())?;
    let label = chosen
        .instance
        .clone()
        .unwrap_or_else(|| "default".to_string());
    Some((label, PathBuf::from(&chosen.session_dir)))
}

fn inbound_topic(instance: &str, suffix: &str) -> String {
    let inst = if instance.is_empty() { "default" } else { instance };
    format!("plugin.inbound.whatsapp.{inst}.pairing.{suffix}")
}

async fn publish_qr(
    broker: &AnyBroker,
    instance: &str,
    challenge_id: Uuid,
    png_b64: String,
    ascii: String,
    expires_at_ms: u64,
) {
    let topic = inbound_topic(instance, "qr");
    let payload = json!({
        "challenge_id": challenge_id.to_string(),
        "png_base64": png_b64,
        "ascii": ascii,
        "expires_at_ms": expires_at_ms,
    });
    let evt = Event::new(&topic, "whatsapp.pairing", payload);
    if let Err(err) = broker.publish(&topic, evt).await {
        tracing::warn!(
            target: "whatsapp.pairing_admin",
            %topic, error = %err,
            "failed to publish pairing QR event"
        );
    }
}

async fn publish_state(
    broker: &AnyBroker,
    instance: &str,
    challenge_id: Uuid,
    state: &str,
    device_jid: Option<&str>,
    error: Option<&str>,
) {
    let topic = inbound_topic(instance, "state");
    let mut payload = json!({
        "challenge_id": challenge_id.to_string(),
        "state": state,
    });
    if let Some(jid) = device_jid {
        payload["device_jid"] = json!(jid);
    }
    if let Some(err) = error {
        payload["error"] = json!(err);
    }
    let evt = Event::new(&topic, "whatsapp.pairing", payload);
    if let Err(err) = broker.publish(&topic, evt).await {
        tracing::warn!(
            target: "whatsapp.pairing_admin",
            %topic, error = %err,
            "failed to publish pairing state event"
        );
    }
}

/// Handle `nexo/admin/whatsapp/pairing/start`. Spawns the
/// wa-agent pump for the requested challenge + instance. Returns
/// ok=true synchronously — QR + terminal state arrive via
/// `plugin.inbound.whatsapp.<inst>.pairing.{qr,state}` broker
/// publications.
pub async fn pairing_start(broker: AnyBroker, request: &Value) -> Value {
    let params = request.get("params").cloned().unwrap_or_default();
    let challenge_id = match parse_challenge_id(&params) {
        Ok(id) => id,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let instance = params
        .get("instance")
        .and_then(Value::as_str)
        .map(str::to_string);

    let (instance_label, session_dir) = match resolve_session_dir(instance.as_deref()).await {
        Some(t) => t,
        None => {
            let label = instance.clone().unwrap_or_else(|| "(default)".into());
            return json!({
                "ok": false,
                "error": format!("instance `{label}` not configured"),
            });
        }
    };

    // Wipe stale signal session so wa-agent always issues a fresh
    // QR. Mirrors the legacy `WhatsappPairingTrigger::start`
    // wipe — without this `connect()` may resume silently and the
    // operator UI never sees a QR. Best-effort: NotFound is the
    // happy path on first pair; other errors logged but
    // not fatal.
    let signal_dir = session_dir.join(".whatsapp-rs");
    match tokio::fs::remove_dir_all(&signal_dir).await {
        Ok(()) => tracing::info!(
            target: "whatsapp.pairing_admin",
            signal_dir = %signal_dir.display(),
            "cleared stale signal session for fresh QR"
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(
            target: "whatsapp.pairing_admin",
            signal_dir = %signal_dir.display(),
            error = %e,
            "could not wipe signal session — pairing may auto-resume"
        ),
    }

    // Cancel + replace any prior in-flight pump for this
    // challenge (idempotent restart).
    if let Some((_, prior)) = pumps().remove(&challenge_id) {
        prior.cancel.cancel();
    }

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let broker_for_task = broker.clone();
    let instance_for_task = instance_label.clone();
    let qr_fired = Arc::new(AtomicBool::new(false));

    let task = tokio::spawn(async move {
        // wa-agent's `pair_with_callback` takes a SYNC FnMut.
        // Bridge to broker publishes via a tokio mpsc so the
        // sync closure stays panic-free.
        let (qr_tx, mut qr_rx) =
            tokio::sync::mpsc::unbounded_channel::<(String, String, u64)>();
        let qr_fired_for_closure = qr_fired.clone();
        let on_qr = move |png_b64: String, ascii: String, expires_at_ms: u64| {
            qr_fired_for_closure.store(true, Ordering::SeqCst);
            let _ = qr_tx.send((png_b64, ascii, expires_at_ms));
        };

        // Drain QR mpsc → broker until the channel closes.
        let broker_for_qr = broker_for_task.clone();
        let instance_for_qr = instance_for_task.clone();
        let qr_publisher = tokio::spawn(async move {
            while let Some((png, ascii, exp)) = qr_rx.recv().await {
                publish_qr(
                    &broker_for_qr,
                    &instance_for_qr,
                    challenge_id,
                    png,
                    ascii,
                    exp,
                )
                .await;
            }
        });

        let outcome = tokio::select! {
            _ = cancel_clone.cancelled() => {
                tracing::info!(
                    target: "whatsapp.pairing_admin",
                    %challenge_id,
                    "pairing pump cancelled by operator"
                );
                qr_publisher.abort();
                pumps().remove(&challenge_id);
                return;
            }
            result = pair_with_callback(&session_dir, on_qr) => result,
        };

        // Allow the mpsc drain to flush any final pending QR
        // event before publishing state.
        tokio::task::yield_now().await;
        qr_publisher.abort();

        match outcome {
            Ok(_) if !qr_fired.load(Ordering::SeqCst) => {
                // `connect()` resolved without firing on_qr — the
                // wipe didn't take and wa-agent silently resumed.
                // Surface Expired + error rather than misleading
                // "linked" with no device bound.
                publish_state(
                    &broker_for_task,
                    &instance_for_task,
                    challenge_id,
                    "expired",
                    None,
                    Some("session_resumed_without_qr — wipe failed; check daemon logs"),
                )
                .await;
                tracing::error!(
                    target: "whatsapp.pairing_admin",
                    %challenge_id,
                    "connect resolved Ok WITHOUT firing on_qr — silent resume defended"
                );
            }
            Ok(_) => {
                publish_state(
                    &broker_for_task,
                    &instance_for_task,
                    challenge_id,
                    "linked",
                    None,
                    None,
                )
                .await;
                tracing::info!(
                    target: "whatsapp.pairing_admin",
                    %challenge_id,
                    "pair_with_callback resolved Ok"
                );
            }
            Err(err) => {
                publish_state(
                    &broker_for_task,
                    &instance_for_task,
                    challenge_id,
                    "expired",
                    None,
                    Some(&err.to_string()),
                )
                .await;
                tracing::warn!(
                    target: "whatsapp.pairing_admin",
                    %challenge_id,
                    error = %err,
                    "pair_with_callback failed"
                );
            }
        }

        pumps().remove(&challenge_id);
    });

    pumps().insert(challenge_id, PumpHandle { cancel, _task: task });

    json!({
        "ok": true,
        "result": { "instance": instance_label, "challenge_id": challenge_id.to_string() },
    })
}

/// Handle `nexo/admin/whatsapp/pairing/cancel`. Aborts the
/// in-flight pump for the requested challenge. Idempotent — a
/// second cancel returns ok=true even when no entry exists, so
/// daemon-side retries don't fail.
pub async fn pairing_cancel(_broker: AnyBroker, request: &Value) -> Value {
    let params = request.get("params").cloned().unwrap_or_default();
    let challenge_id = match parse_challenge_id(&params) {
        Ok(id) => id,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    if let Some((_, h)) = pumps().remove(&challenge_id) {
        h.cancel.cancel();
        tracing::info!(
            target: "whatsapp.pairing_admin",
            %challenge_id,
            "cancelled in-flight pairing pump"
        );
    }
    json!({ "ok": true, "result": { "challenge_id": challenge_id.to_string() } })
}

fn parse_challenge_id(params: &Value) -> Result<Uuid, String> {
    let raw = params
        .get("challenge_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing challenge_id".to_string())?;
    Uuid::parse_str(raw).map_err(|e| format!("invalid challenge_id: {e}"))
}

#[cfg(test)]
fn clear_in_flight() {
    let p = pumps();
    let keys: Vec<Uuid> = p.iter().map(|e| *e.key()).collect();
    for k in keys {
        if let Some((_, h)) = p.remove(&k) {
            h.cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn pairing_start_rejects_missing_challenge_id() {
        clear_in_flight();
        let broker = AnyBroker::local();
        let r = pairing_start(broker, &json!({ "method": "...", "params": {} })).await;
        assert_eq!(r["ok"].as_bool(), Some(false));
        assert!(
            r["error"].as_str().unwrap().contains("missing challenge_id"),
            "expected missing-challenge_id error, got: {r}",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn pairing_start_rejects_invalid_challenge_id() {
        clear_in_flight();
        let broker = AnyBroker::local();
        let r = pairing_start(
            broker,
            &json!({ "params": { "challenge_id": "not-a-uuid" } }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(false));
        assert!(
            r["error"].as_str().unwrap().contains("invalid challenge_id"),
            "expected invalid-challenge_id error, got: {r}",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn pairing_start_rejects_when_no_config_configured() {
        clear_in_flight();
        {
            let mut g = configured_state().write().await;
            *g = Some(Vec::new());
        }
        let broker = AnyBroker::local();
        let cid = Uuid::new_v4();
        let r = pairing_start(
            broker,
            &json!({
                "params": {
                    "challenge_id": cid.to_string(),
                    "instance": "default",
                },
            }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(false));
        assert!(
            r["error"].as_str().unwrap().contains("not configured"),
            "expected `not configured` error, got: {r}",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn pairing_cancel_is_idempotent_when_no_in_flight_entry() {
        clear_in_flight();
        let broker = AnyBroker::local();
        let cid = Uuid::new_v4();
        let r = pairing_cancel(
            broker,
            &json!({ "params": { "challenge_id": cid.to_string() } }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(true));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial]
    async fn pairing_cancel_rejects_invalid_challenge_id() {
        clear_in_flight();
        let broker = AnyBroker::local();
        let r = pairing_cancel(
            broker,
            &json!({ "params": { "challenge_id": "bad" } }),
        )
        .await;
        assert_eq!(r["ok"].as_bool(), Some(false));
    }

    #[test]
    fn parse_challenge_id_round_trips_valid_uuid() {
        let cid = Uuid::new_v4();
        let params = json!({ "challenge_id": cid.to_string() });
        let parsed = parse_challenge_id(&params).unwrap();
        assert_eq!(parsed, cid);
    }

    #[test]
    fn parse_challenge_id_missing_returns_error() {
        let err = parse_challenge_id(&json!({})).unwrap_err();
        assert!(err.contains("missing"));
    }

    #[test]
    fn parse_challenge_id_invalid_returns_error() {
        let err = parse_challenge_id(&json!({ "challenge_id": "abc" })).unwrap_err();
        assert!(err.contains("invalid"));
    }

    #[test]
    fn inbound_topic_uses_default_when_empty_instance() {
        assert_eq!(
            inbound_topic("", "qr"),
            "plugin.inbound.whatsapp.default.pairing.qr"
        );
    }

    #[test]
    fn inbound_topic_preserves_instance_label() {
        assert_eq!(
            inbound_topic("ana", "state"),
            "plugin.inbound.whatsapp.ana.pairing.state"
        );
    }
}
