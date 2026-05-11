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

pub mod bot_registry;
pub mod bridge;
pub mod dispatch;
pub mod env_config;
pub mod events;
pub mod lifecycle;
pub mod media;
pub mod pairing;
pub mod pairing_adapter;
pub mod pairing_trigger;
pub mod plugin;
pub mod session;
pub mod session_id;
#[cfg(not(feature = "embedded"))]
pub mod subprocess_dispatch;
pub mod tool;
pub mod transcriber;

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

use std::sync::Arc;

use nexo_config::WhatsappPluginConfig;
use nexo_core::agent::nexo_plugin_registry::PluginFactory;
use nexo_core::agent::plugin_host::NexoPlugin;

/// Factory builder for one whatsapp plugin instance, used by the
/// in-process embedded path. Multi-account operators call this
/// once per [`WhatsappPluginConfig`] (one per Signal Protocol
/// session_dir / instance label) and register each result in a
/// `PluginFactoryRegistry` under a distinct manifest name;
/// `wire_plugin_registry(..., Some(&factory))` instantiates them
/// on boot.
///
/// Subprocess consumers construct [`WhatsappPlugin`] directly inside
/// `main.rs` from env-derived config and never touch this helper.
pub fn whatsapp_plugin_factory(cfg: WhatsappPluginConfig) -> PluginFactory {
    Box::new(move |_manifest| {
        let plugin: Arc<dyn NexoPlugin> = Arc::new(WhatsappPlugin::new(cfg.clone()));
        Ok(plugin)
    })
}
