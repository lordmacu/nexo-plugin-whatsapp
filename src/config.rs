//! Phase 93.4.b — plugin-owned config types.
//!
//! Until 0.1.8 this plugin re-exported `nexo_config::WhatsappPluginConfig`
//! and its sub-structs. Phase 93 inverts: each plugin owns its
//! config contract (manifest's `[plugin.config_schema]` + this
//! module's Rust definitions); the daemon delivers the operator
//! YAML opaquely via `plugin.configure` JSON-RPC. Dropping the
//! `nexo-config` plugin-types dep cuts the framework→plugin
//! coupling Phase 93 targets.
//!
//! Field shapes mirror `nexo_config::types::plugins::Whatsapp*`
//! verbatim — operator YAML keeps working unchanged.

use serde::Deserialize;

/// Wrapper matching the YAML wire shape (`whatsapp:` top-level key
/// followed by single-account map or array of maps). Plugin tests
/// + the legacy env-config-from-disk migrators consume this.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WhatsappPluginConfigFile {
    pub whatsapp: WhatsappPluginShape,
}

/// Operator YAML accepts either a single map (legacy single-account)
/// or a sequence of maps (multi-account).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum WhatsappPluginShape {
    Single(WhatsappPluginConfig),
    Many(Vec<WhatsappPluginConfig>),
}

impl WhatsappPluginShape {
    pub fn into_vec(self) -> Vec<WhatsappPluginConfig> {
        match self {
            WhatsappPluginShape::Single(c) => vec![c],
            WhatsappPluginShape::Many(v) => v,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WhatsappPluginConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_session_dir")]
    pub session_dir: String,
    #[serde(default = "default_media_dir")]
    pub media_dir: String,
    /// Legacy field; unused at runtime — credentials live under
    /// `session_dir/.whatsapp-rs/creds.json`.
    pub credentials_file: Option<String>,
    #[serde(default)]
    pub acl: WhatsappAclConfig,
    #[serde(default)]
    pub behavior: WhatsappBehaviorConfig,
    #[serde(default)]
    pub rate_limit: WhatsappRateLimitConfig,
    #[serde(default)]
    pub bridge: WhatsappBridgeConfig,
    #[serde(default)]
    pub transcriber: WhatsappTranscriberConfig,
    #[serde(default)]
    pub daemon: WhatsappDaemonConfig,
    #[serde(default)]
    pub public_tunnel: WhatsappPublicTunnelConfig,
    #[serde(default)]
    pub instance: Option<String>,
    #[serde(default)]
    pub allow_agents: Vec<String>,
    #[serde(default)]
    pub typing_mode: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WhatsappPublicTunnelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub only_until_paired: bool,
}

impl Default for WhatsappPublicTunnelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            only_until_paired: true,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WhatsappAclConfig {
    #[serde(default)]
    pub allow_list: Vec<String>,
    #[serde(default = "default_acl_env")]
    pub from_env: String,
}

impl Default for WhatsappAclConfig {
    fn default() -> Self {
        Self {
            allow_list: Vec::new(),
            from_env: default_acl_env(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WhatsappBehaviorConfig {
    #[serde(default = "default_true")]
    pub ignore_chat_meta: bool,
    #[serde(default = "default_true")]
    pub ignore_from_me: bool,
    #[serde(default)]
    pub ignore_groups: bool,
    #[serde(default = "default_skip_backlog_age_secs")]
    pub skip_backlog_age_secs: u64,
}

impl Default for WhatsappBehaviorConfig {
    fn default() -> Self {
        Self {
            ignore_chat_meta: true,
            ignore_from_me: true,
            ignore_groups: false,
            skip_backlog_age_secs: default_skip_backlog_age_secs(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WhatsappRateLimitConfig {
    #[serde(default = "default_rate_global")]
    pub global_per_sec: f32,
    #[serde(default = "default_rate_per_jid")]
    pub per_jid_per_sec: f32,
    #[serde(default = "default_rate_burst")]
    pub burst: u32,
}

impl Default for WhatsappRateLimitConfig {
    fn default() -> Self {
        Self {
            global_per_sec: default_rate_global(),
            per_jid_per_sec: default_rate_per_jid(),
            burst: default_rate_burst(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WhatsappBridgeConfig {
    #[serde(default = "default_response_timeout_ms")]
    pub response_timeout_ms: u64,
    #[serde(default = "default_on_timeout")]
    pub on_timeout: String,
    #[serde(default = "default_apology")]
    pub apology_text: String,
}

impl Default for WhatsappBridgeConfig {
    fn default() -> Self {
        Self {
            response_timeout_ms: default_response_timeout_ms(),
            on_timeout: default_on_timeout(),
            apology_text: default_apology(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WhatsappTranscriberConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_transcriber_skill")]
    pub skill: String,
    #[serde(default = "default_transcriber_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for WhatsappTranscriberConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            skill: default_transcriber_skill(),
            timeout_ms: default_transcriber_timeout_ms(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WhatsappDaemonConfig {
    #[serde(default = "default_true")]
    pub prefer_existing: bool,
}

impl Default for WhatsappDaemonConfig {
    fn default() -> Self {
        Self {
            prefer_existing: true,
        }
    }
}

fn default_enabled() -> bool {
    false
}
fn default_true() -> bool {
    true
}
fn default_session_dir() -> String {
    "./data/whatsapp-session".to_string()
}
fn default_media_dir() -> String {
    "./data/media/whatsapp".to_string()
}
fn default_acl_env() -> String {
    "WA_AGENT_ALLOW".to_string()
}
fn default_rate_global() -> f32 {
    2.0
}
fn default_rate_per_jid() -> f32 {
    1.0
}
fn default_rate_burst() -> u32 {
    5
}
fn default_response_timeout_ms() -> u64 {
    30_000
}
fn default_on_timeout() -> String {
    "noop".to_string()
}
fn default_apology() -> String {
    "Sorry, I took too long to reply. Please try again.".to_string()
}
fn default_transcriber_skill() -> String {
    "whisper".to_string()
}
fn default_transcriber_timeout_ms() -> u64 {
    30_000
}
fn default_skip_backlog_age_secs() -> u64 {
    60
}
