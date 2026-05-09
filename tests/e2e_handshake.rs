//! End-to-end smoke test: spawn the binary, send `initialize`, assert
//! the reply advertises the 4 `whatsapp_*` tools matching the
//! manifest's `[plugin.capabilities.broker]` allowlist + verify
//! `tool.invoke` short-circuits with `-33402 ArgumentInvalid` when
//! the env config is incomplete (no session dir) — happens BEFORE
//! any broker / wa-agent round-trip so the test runs offline.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use serial_test::serial;

const BINARY: &str = env!("CARGO_BIN_EXE_nexo-plugin-whatsapp");

/// Send a JSON-RPC frame + read one reply line.
fn rpc_round_trip(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut BufReader<std::process::ChildStdout>,
    frame: Value,
) -> Value {
    let line = serde_json::to_string(&frame).unwrap();
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
    let mut buf = String::new();
    stdout.read_line(&mut buf).expect("read reply");
    serde_json::from_str(buf.trim()).expect("reply parses as JSON")
}

fn spawn_with_env(env: HashMap<&'static str, &'static str>) -> std::process::Child {
    let mut cmd = Command::new(BINARY);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.spawn().expect("spawn nexo-plugin-whatsapp")
}

fn wait_with_timeout(mut child: std::process::Child, deadline: Duration) {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if start.elapsed() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                return;
            }
        }
    }
}

#[test]
#[serial]
fn initialize_advertises_four_whatsapp_tools() {
    let mut child = spawn_with_env(HashMap::new());
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let reply = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }),
    );

    assert_eq!(reply["jsonrpc"], "2.0");
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["result"]["manifest"]["plugin"]["id"], "whatsapp");

    let tools = reply["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 4, "expected 4 whatsapp_* tools");
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "whatsapp_send_media",
            "whatsapp_send_message",
            "whatsapp_send_reaction",
            "whatsapp_send_reply",
        ]
    );
    for t in tools {
        assert_eq!(
            t["input_schema"]["type"], "object",
            "tool {} must have an object schema",
            t["name"]
        );
    }

    // Clean shutdown.
    let _ = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":{}}),
    );
    drop(stdin);
    wait_with_timeout(child, Duration::from_secs(2));
}

#[test]
#[serial]
fn tool_invoke_without_session_dir_returns_argument_invalid() {
    // No `NEXO_PLUGIN_WHATSAPP_SESSION_DIR` →
    // `whatsapp_config_from_env` returns Err → dispatch maps to
    // `-33402 ArgumentInvalid`. No broker / wa-agent traffic, no
    // plugin.start, fully offline.
    let mut child = spawn_with_env(HashMap::new());
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let _init = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
    );

    let reply = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tool.invoke",
            "params": {
                "plugin_id": "whatsapp",
                "tool_name": "whatsapp_send_message",
                "args": { "to": "+5491100000000", "text": "hi" },
            }
        }),
    );

    assert_eq!(reply["error"]["code"], -33402, "got {reply:?}");
    let msg = reply["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("env config") || msg.contains("SESSION_DIR"),
        "error message should hint at missing env, got: {msg}"
    );

    let _ = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":{}}),
    );
    drop(stdin);
    wait_with_timeout(child, Duration::from_secs(2));
}
