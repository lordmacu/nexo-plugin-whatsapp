# nexo-plugin-whatsapp — operator guide

Standalone repo for the WhatsApp subprocess plugin extracted from
`proyecto/crates/plugins/whatsapp/` in Phase 81.19.a.

## Shape B reminder

Same lib + bin pattern as `nexo-rs-plugin-browser` (81.17.c) and
`nexo-rs-plugin-telegram` (81.18):

- `src/lib.rs` re-exports `WhatsappPlugin` and friends so a
  future embedded build (Phase 90 — Android) can drop the
  subprocess loop and use the plugin in-process.
- `src/main.rs` is the **only** subprocess-specific code. It
  wraps `WhatsappPlugin` in `PluginAdapter`, runs the JSON-RPC
  loop over stdio, and seeds the plugin from the env vars the
  daemon set before spawn. (Subprocess flip = 81.18.b, deferred.)

## Multi-account

A daemon configured with multiple `[plugin.whatsapp]` entries
spawns one binary per instance — not one binary handling all
accounts. Per-instance state (`session_dir`, `media_dir`,
`instance` topic suffix) lives under paths scoped by
the daemon-supplied env vars; the subprocess never enumerates
siblings.

## Signal Protocol session state

`NEXO_PLUGIN_WHATSAPP_SESSION_DIR` points at a directory the
subprocess uses to persist `creds.json` (device identity),
`sessions.json` (double-ratchet state) and `pre-keys/`. Operators
**must NOT** delete this dir while the binary is running —
wa-agent rotates the session credentials on demand and
deleting them mid-flight forces a re-pair (the bot becomes
unreachable until the operator scans a new QR).

If `creds.json` is corrupt at boot, wa-agent writes a backup
(`creds.json.bak.<timestamp>`) and refuses to load — the
plugin emits `InboundEvent::CredentialsExpired` followed by a
fresh `InboundEvent::Qr` once the pairing trigger requeues a
challenge; operator scans the new code from the admin UI to
re-bind.

## QR pairing

`pair_with_callback` (in `session.rs`) hands a synchronous
callback to wa-agent that fires per QR rotation. The callback
in turn pushes the QR string into the daemon's challenge store
+ SSE notifier. The subprocess flip (81.18.b) requires this
callback to round-trip via JSON-RPC notification back to the
parent — tracked as follow-up `81.19.a.pairing-rpc-callback`.

## Debugging tips

- `RUST_LOG=trace,nexo_plugin_whatsapp=trace` exposes every
  bridge wait, every typing presence forward, every Signal
  session state mutation.
- The binary panics on startup if `NEXO_BROKER_URL` is missing;
  that's intentional — running without a broker is silent
  failure.

## TLS caveat (Phase 90 blocker)

`wa-agent` upstream uses `native-tls` (OpenSSL) via its
`reqwest` dep. For an Android NDK build that's a hard blocker
unless the Nexo build system supplies pre-built OpenSSL for the
target. The clean fix is asking wa-agent maintainers to expose
a `rustls-tls` feature flag — tracked as
`81.19.a.tls-rustls`. Until then the binary mixes two TLS
stacks (this repo's reqwest = rustls; wa-agent's = openssl).

## Mining references

Phases that touched the WhatsApp surface: 6.x (initial wire-up),
81.12.c (NexoPlugin trait dual-impl), 82.10.r (typing presence
forwarding), 82.13.x (intervention bridge), 82.15.bx (broker
capability manifest declaration), 81.19.a (this extract).
Per-step rationale lives in `proyecto/PHASES.md` and
`proyecto/PHASES-curated.md`.
