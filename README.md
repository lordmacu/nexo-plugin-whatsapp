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

### Prerequisite — `nexo-rs` daemon

This plugin runs as a subprocess of [`nexo-rs`](https://github.com/lordmacu/nexo-rs).
Install the daemon first:

```bash
curl -fsSL https://lordmacu.github.io/nexo-rs/install.sh | bash
nexo --version   # ≥ 0.1.6
```

Other channels (Docker / Termux / source): see the
[installation guide](https://lordmacu.github.io/nexo-rs/getting-started/installation.html).

### Build the plugin

```bash
cargo build --release
```

`Cargo.lock` is committed — binary repo convention, reproducible
builds from `git checkout v0.1.2 && cargo install --path .`.

## Install into a Nexo daemon

This plugin runs out-of-tree as a stdio subprocess (Phase 81.19.a).
The Nexo daemon discovers it through `plugins.discovery.search_paths`
and spawns it on boot. Until a packaged installer ships, the
operator wires three pieces by hand:

### 1 — Drop the binary + augmented manifest into the discovery dir

The directory you pick must be listed under
`config/plugins/discovery.yaml > discovery.search_paths`. Below
assumes `/var/lib/nexo/plugins/` is in that list; adjust for your
deployment.

```bash
sudo install -d /var/lib/nexo/plugins/whatsapp
sudo install -m 755 target/release/nexo-plugin-whatsapp \
    /var/lib/nexo/plugins/whatsapp/

sudo tee /var/lib/nexo/plugins/whatsapp/plugin.toml >/dev/null <<'TOML'
# Operator-augmented copy of this repo's `nexo-plugin.toml`. The
# upstream manifest is compile-time embedded for the JSON-RPC
# initialize handshake; the on-disk copy here is what the daemon
# walks during discovery, so it MUST add two sections the embedded
# manifest does not carry:
#   - [plugin.entrypoint].command  — absolute path to the binary
#     (the daemon's subprocess.rs does not resolve relative paths
#     against the manifest dir).
#   - [plugin.extends].tools       — allowlist matching every
#     tool name advertised in the initialize reply. Phase 81.29
#     defense rejects subprocess plugins whose handshake claims a
#     tool not in this list.

[plugin]
id              = "whatsapp"
version         = "0.1.3"
name            = "WhatsApp Bot"
description     = "WhatsApp channel plugin via wa-agent (Signal Protocol + Bot API + QR pairing)."
min_nexo_version = ">=0.1.0"

[plugin.requires]
nexo_capabilities = ["broker"]

[plugin.capabilities.broker]
subscribe = ["plugin.outbound.whatsapp", "plugin.outbound.whatsapp.>"]
publish   = ["plugin.inbound.whatsapp",  "plugin.inbound.whatsapp.>"]

[plugin.extends]
tools = [
    "whatsapp_send_message",
    "whatsapp_send_reply",
    "whatsapp_send_reaction",
    "whatsapp_send_media",
]

[plugin.entrypoint]
command = "/var/lib/nexo/plugins/whatsapp/nexo-plugin-whatsapp"
TOML
```

### 2 — Enable the plugin in the daemon config

```yaml
# config/plugins/whatsapp.yaml — daemon reads this and stamps the
# env vars listed under "Daemon wiring" before spawning the binary.
whatsapp:
  enabled: true
  session_dir: /var/lib/nexo/data/whatsapp/session
  media_dir:   /var/lib/nexo/data/whatsapp/media
  instance:    primary
schema_version: 11
```

The `instance` value here becomes the suffix in
`plugin.{inbound,outbound}.whatsapp.<instance>` topics, and
identifies the per-instance subprocess factory the daemon
synthesizes (`whatsapp.primary` in this example).

### 3 — Restart the daemon, pair from the operator UI

```bash
systemctl restart nexo  # or however your deployment respawns nexo
```

On boot you should see in the daemon log:

```
INFO plugins.discovery: plugin registry wire complete loaded=N ...
INFO plugins.init: registered remote tools plugin_id=whatsapp registered_count=4
```

`loaded` must include whatsapp; `registered_count=4` confirms
the four `whatsapp_*` tools are wired into the agent runtime.
If `init_failed_total > 0`, follow up with
`nexo/admin/plugins/doctor` (or grep daemon log for
`plugins.init` WARN lines) — every reject reason lands there
with the manifest path that triggered it.

The first time the plugin runs there are no Signal credentials
on disk, so `whatsapp.start` queues a fresh QR via the daemon's
pairing slot. Trigger pairing from the operator UI
(`/agents/<id>/channels → pair`) and scan the QR with WhatsApp
on your phone. Credentials persist to
`<session_dir>/.whatsapp-rs/creds.json`; subsequent boots
restore the existing session without showing the QR.

### Troubleshooting

The daemon logs every rejection with the plugin's manifest path
and the diagnostic chain. Common failures, in order of operator
likelihood:

| Symptom in `daemon.log` | Cause | Fix |
|---|---|---|
| `whatsapp is configured … but no whatsapp manifest was found` | Binary or `plugin.toml` not in `plugins.discovery.search_paths` | Verify §1 paths against `config/plugins/discovery.yaml` |
| `min_nexo_version >=X.Y.Z does not match current daemon version A.B.C-rc…` | Daemon is on a pre-release; `semver` excludes pre-releases | Run a release build of the daemon, or override `min_nexo_version` to `>=X.Y.Z-0` |
| `initialize reply advertises undeclared tool whatsapp_send_*` | `[plugin.extends].tools` missing in the augmented manifest | Re-paste the §1 manifest; Phase 81.29 defense requires the allowlist |
| `manifest id mismatch: factory expected whatsapp.<instance>, child reported whatsapp` | Daemon predates the synthetic-instance accept fix | Upgrade daemon to a build that tolerates `<base>` as a valid response for `<base>.<instance>` factories |
| `whatsapp_send_message already registered (likely by plugin unknown or built-in)` | Same daemon predates the synthetic-instance skip fix | Same — upgrade daemon |

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
