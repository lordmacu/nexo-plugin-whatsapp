//! Phase 81.19.a — env-var → `WhatsappPluginConfig` reader.
//!
//! The daemon seeds these vars before spawning the subprocess; the
//! plugin never reads YAML directly. Mirrors the
//! `telegram_config_from_env` shape used by `nexo-plugin-telegram`.
//!
//! Today (81.19.a) the daemon imports the lib via path-dep and
//! constructs `WhatsappPlugin::new(cfg)` from the YAML loader, so
//! this helper is forward-looking infra for the deferred
//! subprocess flip (`81.18.b`).

use anyhow::{Context, Result};

use nexo_config::types::plugins::{
    WhatsappAclConfig, WhatsappBehaviorConfig, WhatsappBridgeConfig, WhatsappDaemonConfig,
    WhatsappPluginConfig, WhatsappPublicTunnelConfig, WhatsappRateLimitConfig,
    WhatsappTranscriberConfig,
};

const ENV_INSTANCE: &str = "NEXO_PLUGIN_WHATSAPP_INSTANCE";
const ENV_SESSION_DIR: &str = "NEXO_PLUGIN_WHATSAPP_SESSION_DIR";
const ENV_MEDIA_DIR: &str = "NEXO_PLUGIN_WHATSAPP_MEDIA_DIR";
const ENV_BRIDGE_TIMEOUT_MS: &str = "NEXO_PLUGIN_WHATSAPP_BRIDGE_TIMEOUT_MS";
const ENV_ALLOWLIST: &str = "NEXO_PLUGIN_WHATSAPP_ALLOWLIST";
const ENV_TRANSCRIBE_ENABLED: &str = "NEXO_PLUGIN_WHATSAPP_TRANSCRIBE_ENABLED";
const ENV_WHISPER_TIMEOUT_MS: &str = "NEXO_PLUGIN_WHATSAPP_WHISPER_TIMEOUT_MS";

/// Build a [`WhatsappPluginConfig`] from the daemon-supplied env
/// vars. Fails with an operator-readable hint on the first missing
/// or malformed value so subprocess boot logs name the offender.
pub fn whatsapp_config_from_env() -> Result<WhatsappPluginConfig> {
    let session_dir = std::env::var(ENV_SESSION_DIR).with_context(|| {
        format!("{ENV_SESSION_DIR} missing — daemon must seed the Signal session dir")
    })?;
    if session_dir.trim().is_empty() {
        anyhow::bail!("{ENV_SESSION_DIR} is empty — supply a writable directory path");
    }

    let media_dir = std::env::var(ENV_MEDIA_DIR).with_context(|| {
        format!("{ENV_MEDIA_DIR} missing — daemon must seed the inbound media cache dir")
    })?;
    if media_dir.trim().is_empty() {
        anyhow::bail!("{ENV_MEDIA_DIR} is empty — supply a writable directory path");
    }

    let instance = match std::env::var(ENV_INSTANCE) {
        Ok(s) if !s.trim().is_empty() => Some(s),
        _ => None,
    };

    let allow_list = parse_allowlist()?;
    let bridge_timeout_ms = parse_u64(ENV_BRIDGE_TIMEOUT_MS, 30_000)?;
    let transcribe_enabled = parse_bool(ENV_TRANSCRIBE_ENABLED, false);
    let whisper_timeout_ms = parse_u64(ENV_WHISPER_TIMEOUT_MS, 60_000)?;

    Ok(WhatsappPluginConfig {
        enabled: true,
        session_dir,
        media_dir,
        credentials_file: None,
        acl: WhatsappAclConfig {
            allow_list,
            from_env: String::new(),
        },
        behavior: WhatsappBehaviorConfig::default(),
        rate_limit: WhatsappRateLimitConfig::default(),
        bridge: WhatsappBridgeConfig {
            response_timeout_ms: bridge_timeout_ms,
            on_timeout: "noop".to_string(),
            apology_text: String::new(),
        },
        transcriber: WhatsappTranscriberConfig {
            enabled: transcribe_enabled,
            skill: "whisper".to_string(),
            timeout_ms: whisper_timeout_ms,
        },
        daemon: WhatsappDaemonConfig::default(),
        public_tunnel: WhatsappPublicTunnelConfig::default(),
        instance,
        // Subprocess plugins enforce the agent allowlist via the
        // resolver's `credentials.whatsapp` binding upstream
        // (daemon-side). Leaving the per-plugin override empty
        // keeps that single-source-of-truth.
        allow_agents: Vec::new(),
        typing_mode: Default::default(),
    })
}

fn parse_u64(var: &str, default: u64) -> Result<u64> {
    match std::env::var(var) {
        Ok(s) if !s.trim().is_empty() => s
            .trim()
            .parse::<u64>()
            .with_context(|| format!("{var}={s:?} is not a non-negative integer")),
        _ => Ok(default),
    }
}

fn parse_bool(var: &str, default: bool) -> bool {
    match std::env::var(var) {
        Ok(s) => matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => default,
    }
}

fn parse_allowlist() -> Result<Vec<String>> {
    match std::env::var(ENV_ALLOWLIST) {
        Ok(s) if !s.trim().is_empty() => {
            let jids: Vec<String> = serde_json::from_str(&s).with_context(|| {
                format!("{ENV_ALLOWLIST}={s:?} must be a JSON array of E.164 phone strings")
            })?;
            Ok(jids)
        }
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn clear_all() {
        for var in [
            ENV_INSTANCE,
            ENV_SESSION_DIR,
            ENV_MEDIA_DIR,
            ENV_BRIDGE_TIMEOUT_MS,
            ENV_ALLOWLIST,
            ENV_TRANSCRIBE_ENABLED,
            ENV_WHISPER_TIMEOUT_MS,
        ] {
            std::env::remove_var(var);
        }
    }

    #[test]
    #[serial]
    fn config_happy_path() {
        clear_all();
        std::env::set_var(ENV_SESSION_DIR, "/tmp/wa-session");
        std::env::set_var(ENV_MEDIA_DIR, "/tmp/wa-media");
        std::env::set_var(ENV_INSTANCE, "ventas");
        std::env::set_var(ENV_ALLOWLIST, r#"["+5491100000000", "+5491111111111"]"#);
        std::env::set_var(ENV_BRIDGE_TIMEOUT_MS, "45000");

        let cfg = whatsapp_config_from_env().expect("happy path");
        assert_eq!(cfg.session_dir, "/tmp/wa-session");
        assert_eq!(cfg.media_dir, "/tmp/wa-media");
        assert_eq!(cfg.instance.as_deref(), Some("ventas"));
        assert_eq!(
            cfg.acl.allow_list,
            vec!["+5491100000000".to_string(), "+5491111111111".to_string()]
        );
        assert_eq!(cfg.bridge.response_timeout_ms, 45_000);
        assert!(!cfg.transcriber.enabled);
        clear_all();
    }

    #[test]
    #[serial]
    fn config_missing_session_dir_errors() {
        clear_all();
        std::env::set_var(ENV_MEDIA_DIR, "/tmp/wa-media");
        let err = whatsapp_config_from_env().unwrap_err();
        assert!(
            err.to_string().contains(ENV_SESSION_DIR),
            "error must name the missing var, got: {err}"
        );
        clear_all();
    }

    #[test]
    #[serial]
    fn config_invalid_allowlist_json_errors() {
        clear_all();
        std::env::set_var(ENV_SESSION_DIR, "/tmp/wa-session");
        std::env::set_var(ENV_MEDIA_DIR, "/tmp/wa-media");
        std::env::set_var(ENV_ALLOWLIST, "[bad json");
        let err = whatsapp_config_from_env().unwrap_err();
        assert!(
            err.to_string().contains(ENV_ALLOWLIST),
            "error must name the offending var, got: {err}"
        );
        clear_all();
    }

    #[test]
    #[serial]
    fn config_empty_session_dir_errors() {
        clear_all();
        std::env::set_var(ENV_SESSION_DIR, "   ");
        std::env::set_var(ENV_MEDIA_DIR, "/tmp/wa-media");
        let err = whatsapp_config_from_env().unwrap_err();
        assert!(err.to_string().contains("empty"), "got: {err}");
        clear_all();
    }
}
