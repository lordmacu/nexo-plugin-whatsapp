# nexo-plugin-whatsapp

WhatsApp bot channel plugin for the [Nexo agent framework][nexo].
Wraps the [`wa-agent`][wa-agent] crate (Signal Protocol + QR pairing
+ Bot API) and ships as a `lib + bin` Shape B package per Phase
81.19.a:

- **lib** — re-exports `WhatsappPlugin`, the pairing trigger /
  adapter, the 6 `whatsapp_*` tool defs, and the inbound event
  enum so a future embedded build (Phase 90 — Android) can pull
  the plugin straight out of the lib surface. The Nexo daemon
  imports this lib via path-dep today (subprocess flip is the
  deferred follow-up `81.18.b`, shared with telegram).

- **bin** — `nexo-plugin-whatsapp` runs the JSON-RPC dispatch
  loop on stdio via
  `nexo_microapp_sdk::plugin::PluginAdapter`. The daemon
  spawns this binary per `plugin.whatsapp[]` instance once
  81.18.b lands.

Out-of-tree per **Phase 81.19.a**: extracted from
`proyecto/crates/plugins/whatsapp/` so the plugin can ship and
upgrade independently of the framework, and so a future Android
embedded build can drop the subprocess loop and re-use
`WhatsappPlugin` in-process.

## Layout

```
nexo-rs-plugin-whatsapp/
├── Cargo.toml             # lib + [[bin]], path-deps interim
├── nexo-plugin.toml       # manifest, [plugin.capabilities.broker]
├── src/
│   ├── lib.rs                  # re-exports for embedded consumers
│   ├── main.rs                 # subprocess entrypoint
│   ├── env_config.rs           # env-var → WhatsappPluginConfig
│   ├── subprocess_dispatch.rs  # tool.invoke → Plugin::send_command
│   ├── plugin.rs               # WhatsappPlugin (verbatim)
│   ├── bridge.rs               # bridge handler (verbatim)
│   ├── dispatch.rs             # outbound dispatcher (verbatim)
│   ├── events.rs               # InboundEvent (verbatim)
│   ├── lifecycle.rs            # event forwarder + presence (verbatim)
│   ├── media.rs                # MIME→variant + downloads (verbatim)
│   ├── pairing.rs              # QrSnapshot + dispatch_route (verbatim)
│   ├── pairing_adapter.rs      # PairingChannelAdapter (verbatim)
│   ├── pairing_trigger.rs      # admin RPC bridge (verbatim)
│   ├── session.rs              # pair_with_callback (verbatim)
│   ├── session_id.rs           # UUIDv5 session id (verbatim)
│   ├── tool.rs                 # 6 tool defs + handlers (verbatim)
│   ├── transcriber.rs          # whisper subprocess wrapper (verbatim)
│   └── bot_registry.rs         # admin RPC session lookup (verbatim)
└── tests/                      # 4 ported + 1 e2e handshake
```

## Build

```bash
cargo build --release
```

`Cargo.lock` is committed — binary repo convention, reproducible
builds from `git checkout v0.1.2 && cargo install --path .`.

## Daemon wiring

The daemon spawns this binary per `plugin.whatsapp[]` config entry
(once 81.18.b lands) and seeds it with the env vars below. None
of these are read from disk; the daemon is the single source of
truth for runtime config.

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `NEXO_PLUGIN_WHATSAPP_INSTANCE`            | no  | `""` | topic suffix; empty = legacy single-account |
| `NEXO_PLUGIN_WHATSAPP_SESSION_DIR`         | yes | — | Signal Protocol creds + sessions + pre-keys |
| `NEXO_PLUGIN_WHATSAPP_MEDIA_DIR`           | yes | — | inbound media downloads |
| `NEXO_PLUGIN_WHATSAPP_BRIDGE_TIMEOUT_MS`   | no  | `30000` | bridge wait for matched reply |
| `NEXO_PLUGIN_WHATSAPP_ALLOWLIST`           | no  | `[]` | JSON array of E.164 phone numbers; empty = no allowlist |
| `NEXO_PLUGIN_WHATSAPP_TRANSCRIBE_ENABLED`  | no  | `false` | voice note auto-transcribe |
| `NEXO_PLUGIN_WHATSAPP_WHISPER_COMMAND`     | no  | `./extensions/openai-whisper/...` | whisper binary path |
| `NEXO_PLUGIN_WHATSAPP_WHISPER_TIMEOUT_MS`  | no  | `60000` | transcribe deadline |
| `NEXO_BROKER_URL`                          | yes | — | NATS endpoint (already global) |
| `RUST_LOG`                                 | no  | `info` | tracing filter |

Multi-account: spawn one binary per instance. Topics, session
dir and media dir are scoped per `INSTANCE` so the binaries
don't contend on shared state. Daemon-side `81.18.b` needs to
generalize the existing single-instance env seeding to N
spawns.

## Topics

- `plugin.inbound.whatsapp.<instance>` — `InboundEvent` payload
  (WhatsApp → agent)
- `plugin.outbound.whatsapp.<instance>` — `Command` payload
  (agent → WhatsApp)
- Legacy single-account (no instance): `plugin.inbound.whatsapp` /
  `plugin.outbound.whatsapp`

## TLS caveat

`wa-agent` upstream uses `native-tls` (OpenSSL) via its `reqwest`
dep; this repo's `reqwest` direct dep uses `rustls-tls`. Both
TLS stacks live in the same binary, slightly bloating size. A
proper resolution requires `wa-agent` to expose a `rustls-tls`
feature flag — tracked as follow-up `81.19.a.tls-rustls`. For
the Android NDK build (Phase 90) the OpenSSL system header
requirement is the headline blocker; reach out to the wa-agent
maintainer before pinning Phase 90 timeline.

## Path-dep disclaimer

Until the proyecto-side crates land on crates.io, every `cargo
build` of this repo expects the layout

```
~/chat/
├── nexo-rs-plugin-whatsapp/   ← this repo
└── proyecto/                  ← Nexo framework workspace
    └── crates/{microapp-sdk,broker,core,config,llm,auth,pairing,plugin-manifest,tool-meta}/
```

If `proyecto/` isn't adjacent, override the path-deps in your
local `Cargo.toml` or wait for the Phase 81.18.c crates.io
publish wave.

[nexo]: https://github.com/lordmacu/nexo-rs
[wa-agent]: https://github.com/lordmacu/wa-agent
