//! Subprocess `tool.invoke` dispatcher.
//!
//! Maps the 4 tool names declared by [`whatsapp_tool_defs`] to
//! [`WhatsappPlugin::send_command`] calls. The in-tree handlers in
//! [`tool`] publish to the broker via `publish_outbound`; the
//! subprocess path uses the same plugin API — `send_command` itself
//! publishes to the broker which the plugin's own dispatcher
//! consumes — so the wire surface is byte-equivalent.
//!
//! [`tool`]: crate::tool

use nexo_core::agent::plugin::{Command, Plugin, Response};
use nexo_microapp_sdk::plugin::{ToolDef as SdkToolDef, ToolInvocation, ToolInvocationError};
use serde_json::{json, Value};

use crate::plugin::WhatsappPlugin;
use crate::tool::{
    WhatsappSendMediaTool, WhatsappSendMessageTool, WhatsappSendReactionTool, WhatsappSendReplyTool,
};

/// Map an in-tree `nexo_llm::ToolDef` (used by the broker-published
/// handlers in [`tool`]) to the subprocess SDK shape declared in
/// `initialize`. Field rename: `parameters` → `input_schema`.
///
/// [`tool`]: crate::tool
fn to_sdk_def(d: nexo_llm::ToolDef) -> SdkToolDef {
    SdkToolDef {
        name: d.name,
        description: d.description,
        input_schema: d.parameters,
    }
}

/// The full list of tool defs the subprocess advertises in its
/// `initialize` reply. Daemon-side `RemoteToolHandler` validates
/// each name against the manifest's tools allowlist before
/// exposing them to agents.
pub fn whatsapp_tool_defs() -> Vec<SdkToolDef> {
    [
        WhatsappSendMessageTool::tool_def(),
        WhatsappSendReplyTool::tool_def(),
        WhatsappSendReactionTool::tool_def(),
        WhatsappSendMediaTool::tool_def(),
    ]
    .into_iter()
    .map(to_sdk_def)
    .collect()
}

/// Route a single `tool.invoke` request through the
/// [`WhatsappPlugin::send_command`] API. Caller is responsible
/// for resolving the live plugin (the subprocess holds one via
/// `OnceCell`) and passing it here.
///
/// Returns the JSON value placed in the JSON-RPC `result` field.
/// Errors map to `ToolInvocationError`:
///   * malformed args → [`ToolInvocationError::ArgumentInvalid`]
///   * broker publish failure → [`ToolInvocationError::ExecutionFailed`]
///   * plugin not started → [`ToolInvocationError::Unavailable`]
pub async fn dispatch_whatsapp_tool(
    plugin: &WhatsappPlugin,
    invocation: ToolInvocation,
) -> Result<Value, ToolInvocationError> {
    let args = invocation.args;
    let cmd = match invocation.tool_name.as_str() {
        "whatsapp_send_message" => {
            let to = require_str(&args, "to")?;
            let text = require_str(&args, "text")?;
            if text.is_empty() {
                return Err(ToolInvocationError::ArgumentInvalid(
                    "`text` must not be empty".into(),
                ));
            }
            Command::SendMessage {
                to: to.to_string(),
                text: text.to_string(),
            }
        }
        "whatsapp_send_reply" => {
            let to = require_str(&args, "to")?;
            let msg_id = require_str(&args, "msg_id")?;
            let text = require_str(&args, "text")?;
            Command::Custom {
                name: "reply".to_string(),
                payload: json!({"to": to, "msg_id": msg_id, "text": text}),
            }
        }
        "whatsapp_send_reaction" => {
            let to = require_str(&args, "to")?;
            let msg_id = require_str(&args, "msg_id")?;
            let emoji = args.get("emoji").and_then(|v| v.as_str()).unwrap_or("");
            Command::Custom {
                name: "react".to_string(),
                payload: json!({"to": to, "msg_id": msg_id, "emoji": emoji}),
            }
        }
        "whatsapp_send_media" => {
            let to = require_str(&args, "to")?;
            let url = require_str(&args, "url")?;
            let caption = args.get("caption").and_then(|v| v.as_str()).unwrap_or("");
            let file_name = args.get("file_name").and_then(|v| v.as_str());
            // `Command::SendMedia` carries `to`/`url`/`caption` only;
            // `file_name` lacks a slot today, so any optional name
            // falls back to a `Command::Custom("media", ...)` shape
            // the dispatcher's "media" branch consumes. Without a
            // file name the typed variant is preferable for tracing.
            if let Some(name) = file_name {
                Command::Custom {
                    name: "media".to_string(),
                    payload: json!({
                        "to": to,
                        "url": url,
                        "caption": caption,
                        "file_name": name,
                    }),
                }
            } else {
                Command::SendMedia {
                    to: to.to_string(),
                    url: url.to_string(),
                    caption: if caption.is_empty() {
                        None
                    } else {
                        Some(caption.to_string())
                    },
                }
            }
        }
        other => return Err(ToolInvocationError::NotFound(other.to_string())),
    };

    match plugin.send_command(cmd).await {
        Ok(Response::MessageSent { message_id }) => {
            Ok(json!({"ok": true, "message_id": message_id}))
        }
        Ok(Response::Ok) => Ok(json!({"ok": true})),
        Ok(Response::Error { message }) => Ok(json!({"ok": false, "error": message})),
        Ok(Response::Custom { payload }) => Ok(payload),
        Err(e) => {
            let s = e.to_string();
            if s.contains("not started") {
                Err(ToolInvocationError::Unavailable(s))
            } else {
                Err(ToolInvocationError::ExecutionFailed(s))
            }
        }
    }
}

fn require_str<'a>(args: &'a Value, field: &str) -> Result<&'a str, ToolInvocationError> {
    args.get(field).and_then(|v| v.as_str()).ok_or_else(|| {
        ToolInvocationError::ArgumentInvalid(format!("`{field}` is required (string)"))
    })
}
