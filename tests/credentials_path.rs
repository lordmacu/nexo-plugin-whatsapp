//! Phase 93.8.b — coverage for the on_credentials_* handler logic
//! (handlers live inside `main.rs` and aren't directly callable
//! from integration tests; these tests exercise the same
//! `configured_state()`-backed lookup logic inline).

use nexo_plugin_whatsapp::{configured_state, WhatsappPluginConfig};
use serial_test::serial;

fn parse_cfg(yaml: &str) -> Vec<WhatsappPluginConfig> {
    let value: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
    serde_yaml::from_value(value).unwrap()
}

async fn list_handler() -> Vec<String> {
    let guard = configured_state().read().await;
    guard
        .as_ref()
        .map(|v| v.iter().filter_map(|c| c.instance.clone()).collect())
        .unwrap_or_default()
}

async fn issue_handler(account_id: &str, agent_id: &str) -> Result<(), String> {
    let guard = configured_state().read().await;
    let Some(cfgs) = guard.as_ref() else {
        return Err("not_found".to_string());
    };
    let cfg = cfgs
        .iter()
        .find(|c| c.instance.as_deref() == Some(account_id));
    match cfg {
        None => Err("not_found".to_string()),
        Some(c) if c.allow_agents.is_empty() || c.allow_agents.contains(&agent_id.to_string()) => {
            Ok(())
        }
        Some(_) => Err("not_permitted".to_string()),
    }
}

async fn resolve_bytes_handler(account_id: &str) -> Result<Vec<u8>, String> {
    let guard = configured_state().read().await;
    let Some(cfgs) = guard.as_ref() else {
        return Err("not_found".to_string());
    };
    let cfg = cfgs
        .iter()
        .find(|c| c.instance.as_deref() == Some(account_id))
        .ok_or_else(|| "not_found".to_string())?;
    serde_json::to_vec(cfg).map_err(|e| format!("serialize failed: {e}"))
}

#[tokio::test]
#[serial]
async fn list_returns_configured_instance_names() {
    let cfgs = parse_cfg(
        r#"
- session_dir: ./data/wa-main
  instance: main
- session_dir: ./data/wa-work
  instance: work
"#,
    );
    *configured_state().write().await = Some(cfgs);
    let accounts = list_handler().await;
    assert_eq!(accounts.len(), 2);
    assert!(accounts.contains(&"main".to_string()));
    assert!(accounts.contains(&"work".to_string()));
    *configured_state().write().await = None;
}

#[tokio::test]
#[serial]
async fn issue_permits_when_allow_agents_empty() {
    let cfgs = parse_cfg(
        r#"
- session_dir: ./data/wa-main
  instance: main
"#,
    );
    *configured_state().write().await = Some(cfgs);
    issue_handler("main", "alice").await.expect("accepted");
    *configured_state().write().await = None;
}

#[tokio::test]
#[serial]
async fn issue_rejects_when_account_not_found() {
    *configured_state().write().await = None;
    let err = issue_handler("nonexistent", "alice")
        .await
        .expect_err("expected not_found");
    assert_eq!(err, "not_found");
}

#[tokio::test]
#[serial]
async fn issue_rejects_when_allow_agents_excludes() {
    let cfgs = parse_cfg(
        r#"
- session_dir: ./data/wa-main
  instance: main
  allow_agents: ["bob"]
"#,
    );
    *configured_state().write().await = Some(cfgs);
    let err = issue_handler("main", "alice")
        .await
        .expect_err("expected not_permitted");
    assert_eq!(err, "not_permitted");
    *configured_state().write().await = None;
}

#[tokio::test]
#[serial]
async fn resolve_bytes_returns_serde_json_encoded_config() {
    let cfgs = parse_cfg(
        r#"
- session_dir: ./data/wa-main
  instance: main
"#,
    );
    *configured_state().write().await = Some(cfgs);
    let bytes = resolve_bytes_handler("main").await.expect("resolve ok");
    let decoded: WhatsappPluginConfig =
        serde_json::from_slice(&bytes).expect("round-trip");
    assert_eq!(decoded.instance.as_deref(), Some("main"));
    assert_eq!(decoded.session_dir, "./data/wa-main");
    *configured_state().write().await = None;
}
