//! `nexo-plugin-whatsapp` — WhatsApp channel plugin backed by the
//! `wa-agent` crate (a.k.a. `whatsapp_rs` on the imports side).
//!
//! See `proyecto/docs/wa-agent-integration.md` for the integration
//! ADR. Extracted out-of-tree so a future embedded build (Android)
//! can drop the subprocess loop and re-use `WhatsappPlugin`
//! in-process via the lib re-exports below.
//!
//! ## Two consumers
//!
//! 1. **Subprocess (default)** — `src/main.rs`
//!    wraps [`WhatsappPlugin`] in
//!    [`nexo_microapp_sdk::plugin::PluginAdapter`] and runs the
//!    JSON-RPC dispatch loop over stdio. The daemon spawns one
//!    binary per `plugin.whatsapp[]` instance and seeds it via
//!    env vars; per-instance Signal Protocol state is fully owned
//!    by the subprocess, no cross-process coordination.
//!
//! 2. **Embedded / in-process (future mobile)** — a host process
//!    imports the lib directly and instantiates [`WhatsappPlugin`]
//!    in-process via [`whatsapp_plugin_factory`] or
//!    `WhatsappPlugin::new(cfg)`. The `embedded` cargo feature drops
//!    subprocess code paths so the resulting binary stays lean.

pub mod auto_discovery;
pub mod bot_registry;
pub mod bridge;
pub mod config;
pub mod configured_state;
pub mod dispatch;
pub mod env_config;
pub mod events;
pub mod lifecycle;
pub mod media;
pub mod pairing;
pub mod pairing_adapter;
pub mod pairing_admin;
pub mod pairing_trigger;
pub mod plugin;
pub mod session;
pub mod session_id;
#[cfg(not(feature = "embedded"))]
pub mod subprocess_dispatch;
pub mod tool;
pub mod transcriber;

pub use config::{
    WhatsappAclConfig, WhatsappBehaviorConfig, WhatsappBridgeConfig, WhatsappDaemonConfig,
    WhatsappPluginConfig, WhatsappPluginConfigFile, WhatsappPluginShape,
    WhatsappPublicTunnelConfig, WhatsappRateLimitConfig, WhatsappTranscriberConfig,
};
pub use configured_state::configured_state;
pub use env_config::whatsapp_config_from_env;
pub use events::InboundEvent;
pub use pairing::{dispatch_route, QrSnapshot, SharedPairingState, StatusSnapshot, WhatsappRoute};
pub use pairing_adapter::WhatsappPairingAdapter;
pub use pairing_trigger::{WhatsappPairingTrigger, CHANNEL_ID};
pub use plugin::WhatsappPlugin;
pub use session_id::session_id_for_jid;
#[cfg(not(feature = "embedded"))]
pub use subprocess_dispatch::{dispatch_whatsapp_tool, whatsapp_tool_defs};
pub use tool::register_whatsapp_tools;

// Phase 93.4.b — legacy `whatsapp_plugin_factory(cfg)` factory
// removed; subprocess auto-factory replaces it. Local config
// types own the shape (see `config` + `configured_state` modules).
