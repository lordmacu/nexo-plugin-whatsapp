# Changelog

## 0.3.0 — 2026-05-15

### Added

- Manifest declares `[plugin.credentials_schema]` (Phase 93.8.a-daemon)
  with `enabled = true` + `accounts_shape = "array"`. Daemon's
  `SubprocessNexoPlugin::credential_store()` reads this section
  and instantiates a `RemoteCredentialStore` round-tripping the
  four `plugin.credentials.*` JSON-RPCs.
- SDK `on_credentials_list` / `on_credentials_issue` /
  `on_credentials_resolve_bytes` / `on_credentials_reload`
  handlers registered in `main.rs`, all backed by
  `configured_state()`. List returns instance names, issue
  enforces `allow_agents` allowlist, resolve_bytes returns the
  serde_json-encoded `WhatsappPluginConfig`.
- `WhatsappPluginConfig` + sub-structs now derive `Serialize` so
  the resolve_bytes handler can round-trip through serde_json.

### Tests

- `tests/credentials_path.rs` — 5 integration tests covering
  list / issue allow-list paths / not-found / resolve_bytes
  round-trip.

## 0.2.0 — 2026-05-15

### Breaking

- Plugin owns its config types. `nexo_config::WhatsappPluginConfig`
  + sub-structs (`WhatsappPublicTunnelConfig`, `WhatsappAclConfig`,
  `WhatsappBehaviorConfig`, `WhatsappRateLimitConfig`,
  `WhatsappBridgeConfig`, `WhatsappTranscriberConfig`,
  `WhatsappDaemonConfig`) no longer come from `nexo-config`;
  equivalent definitions live in `nexo_plugin_whatsapp::config`.
  Field shapes byte-for-byte identical so operator YAML keeps
  working.
- `whatsapp_plugin_factory(cfg)` factory function removed.
  Subprocess auto-factory replaces it.
- Manifest version bumped `"0.1.8" → "0.2.0"` to match crate.

### Added

- Manifest declares `[plugin.config_schema]` (Phase 93.1) with
  `shape = "array"` + full JSON Schema covering acl / behavior /
  rate_limit / bridge / transcriber / daemon / public_tunnel
  knobs.
- SDK `on_configure(...)` handler (Phase 93.4.a-sdk) receives
  operator YAML via `plugin.configure` JSON-RPC (Phase 93.2);
  caches `Vec<WhatsappPluginConfig>` via the new
  `configured_state()` accessor.
- `shared_plugin()` prefers configured state; falls back to
  `whatsapp_config_from_env()` env-var path during the 0.2.x
  deprecation window.
- 5 new integration tests in `tests/configure_path.rs` —
  deserialise / unknown-field rejection / hot-reload re-call /
  env fallback / precedence (configure-wins-over-env).

### Backward compatibility

- Env-var fallback (`NEXO_PLUGIN_WHATSAPP_*` vars) keeps working
  when daemon doesn't deliver `plugin.configure`. Removed in a
  future 0.3.0 once Phase 93.5 closes the daemon-side
  typed-fields deprecation window.

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
