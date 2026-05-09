# Changelog

## 0.1.2 — 2026-05-09

Initial extract from `proyecto/crates/plugins/whatsapp/` per Phase
81.19.a. Sources copied verbatim; no behavioural change.

Surface re-exports from `lib.rs`:

- `WhatsappPlugin`
- `WhatsappPairingAdapter` (broker-side adapter for daemon
  in-process use)
- `WhatsappPairingTrigger`, `pairing_trigger::CHANNEL_ID`
  (admin RPC trigger)
- `pairing::{SharedPairingState, dispatch_route, WhatsappRoute,
  QrSnapshot, StatusSnapshot}`
- `events::InboundEvent`
- `register_whatsapp_tools`
- `whatsapp_plugin_factory` (embedded path consumer)
- `session_id_for_jid` (UUIDv5 deterministic)
- `whatsapp_config_from_env`,
  `dispatch_whatsapp_tool`, `whatsapp_tool_defs`
  (subprocess path; gated off `embedded` feature)

`main.rs` is new: wraps the plugin in
`nexo_microapp_sdk::plugin::PluginAdapter` and runs the JSON-RPC
loop over stdio. Manifest is bundled at compile-time via
`include_str!("../nexo-plugin.toml")`.

The Nexo daemon imports the lib via path-dep
(`nexo-plugin-whatsapp = { path = "../nexo-rs-plugin-whatsapp" }`)
so today's in-tree behaviour is byte-equivalent. Subprocess flip
is the deferred follow-up `81.18.b`, shared with telegram.
