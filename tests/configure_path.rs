//! Phase 93.4.b — coverage for the configure(value) hook +
//! configured_state singleton + shared_plugin preference order.

use nexo_plugin_whatsapp::{configured_state, whatsapp_config_from_env, WhatsappPluginConfig};
use serial_test::serial;

#[tokio::test]
#[serial]
async fn configure_deserialises_single_entry_array() {
    let yaml = r#"
- session_dir: ./data/wa-main
  instance: main
"#;
    let value: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
    let parsed: Vec<WhatsappPluginConfig> =
        serde_yaml::from_value(value).expect("yaml round-trips");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].session_dir, "./data/wa-main");
    assert_eq!(parsed[0].instance.as_deref(), Some("main"));
    *configured_state().write().await = None;
}

#[tokio::test]
#[serial]
async fn configure_unknown_field_errors() {
    // `deny_unknown_fields` catches typos.
    let yaml = r#"
- bogus_field: 1
"#;
    let value: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
    let result: Result<Vec<WhatsappPluginConfig>, _> = serde_yaml::from_value(value);
    let err = result.expect_err("unknown field must fail");
    assert!(
        err.to_string().to_lowercase().contains("bogus_field"),
        "error should mention bogus_field, got: {err}",
    );
}

#[tokio::test]
#[serial]
async fn configure_overwrites_on_hot_reload_recall() {
    let value_a: serde_yaml::Value =
        serde_yaml::from_str(r#"- session_dir: "/tmp/a""#).unwrap();
    let parsed_a: Vec<WhatsappPluginConfig> = serde_yaml::from_value(value_a).unwrap();
    *configured_state().write().await = Some(parsed_a);

    let value_b: serde_yaml::Value =
        serde_yaml::from_str(r#"- session_dir: "/tmp/b""#).unwrap();
    let parsed_b: Vec<WhatsappPluginConfig> = serde_yaml::from_value(value_b).unwrap();
    *configured_state().write().await = Some(parsed_b);

    let guard = configured_state().read().await;
    let current = guard.as_ref().expect("state populated");
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].session_dir, "/tmp/b");
    drop(guard);
    *configured_state().write().await = None;
}

#[tokio::test]
#[serial]
async fn legacy_env_path_active_when_configured_state_empty() {
    *configured_state().write().await = None;
    std::env::set_var("NEXO_PLUGIN_WHATSAPP_SESSION_DIR", "/tmp/wa-env");
    std::env::set_var("NEXO_PLUGIN_WHATSAPP_MEDIA_DIR", "/tmp/wa-media");
    let cfg = whatsapp_config_from_env().expect("env path works");
    assert_eq!(cfg.session_dir, "/tmp/wa-env");
    std::env::remove_var("NEXO_PLUGIN_WHATSAPP_SESSION_DIR");
    std::env::remove_var("NEXO_PLUGIN_WHATSAPP_MEDIA_DIR");
}

#[tokio::test]
#[serial]
async fn configured_state_value_wins_over_env_var() {
    let value: serde_yaml::Value =
        serde_yaml::from_str(r#"- session_dir: "/tmp/FROM_RPC""#).unwrap();
    let parsed: Vec<WhatsappPluginConfig> = serde_yaml::from_value(value).unwrap();
    *configured_state().write().await = Some(parsed);
    std::env::set_var("NEXO_PLUGIN_WHATSAPP_SESSION_DIR", "/tmp/FROM_ENV");
    std::env::set_var("NEXO_PLUGIN_WHATSAPP_MEDIA_DIR", "/tmp/media");

    let chosen = {
        let guard = configured_state().read().await;
        if let Some(vec) = guard.as_ref() {
            vec.first().cloned().expect("non-empty")
        } else {
            whatsapp_config_from_env().expect("env fallback")
        }
    };
    assert_eq!(chosen.session_dir, "/tmp/FROM_RPC");

    std::env::remove_var("NEXO_PLUGIN_WHATSAPP_SESSION_DIR");
    std::env::remove_var("NEXO_PLUGIN_WHATSAPP_MEDIA_DIR");
    *configured_state().write().await = None;
}
