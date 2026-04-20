use chrono::Utc;
use codex_git_utils::get_git_repo_root;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use uuid::Uuid;

pub(crate) const NATIVE_OBSERVE_CAPABILITIES: &[&str] = &[
    "assistant_result",
    "structured_post_tool_payload",
    "query_aware_context",
    "event_identity",
];
const NATIVE_OBSERVE_PAYLOAD_VERSION: &str = "1";
const NATIVE_OBSERVE_SOURCE: &str = "codex-native";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PersistenceClass {
    Persistent,
    Ephemeral,
    DiagnosticsOnly,
}

impl PersistenceClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Persistent => "persistent",
            Self::Ephemeral => "ephemeral",
            Self::DiagnosticsOnly => "diagnostics_only",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookFamily {
    SessionStart,
    PromptSubmit,
    PreToolUse,
    PostToolUse,
    PostToolFailure,
    AssistantResult,
    SubagentStart,
    SubagentStop,
    Stop,
    Notification,
    TaskCompleted,
    SessionEnd,
}

impl HookFamily {
    fn from_event_name(event_name: &str) -> Option<Self> {
        match event_name {
            "SessionStart" => Some(Self::SessionStart),
            "UserPromptSubmit" => Some(Self::PromptSubmit),
            "PreToolUse" => Some(Self::PreToolUse),
            "PostToolUse" => Some(Self::PostToolUse),
            "PostToolUseFailure" => Some(Self::PostToolFailure),
            "AssistantResult" => Some(Self::AssistantResult),
            "SubagentStart" => Some(Self::SubagentStart),
            "SubagentStop" => Some(Self::SubagentStop),
            "Stop" => Some(Self::Stop),
            "Notification" => Some(Self::Notification),
            "TaskCompleted" => Some(Self::TaskCompleted),
            "SessionEnd" => Some(Self::SessionEnd),
            _ => None,
        }
    }

    fn hook_type(self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::PromptSubmit => "prompt_submit",
            Self::PreToolUse => "pre_tool_use",
            Self::PostToolUse => "post_tool_use",
            Self::PostToolFailure => "post_tool_failure",
            Self::AssistantResult => "assistant_result",
            Self::SubagentStart => "subagent_start",
            Self::SubagentStop => "subagent_stop",
            Self::Stop => "stop",
            Self::Notification => "notification",
            Self::TaskCompleted => "task_completed",
            Self::SessionEnd => "session_end",
        }
    }

    fn persistence_class(self, data: &Value) -> PersistenceClass {
        match self {
            Self::PromptSubmit
            | Self::PostToolUse
            | Self::PostToolFailure
            | Self::AssistantResult => PersistenceClass::Persistent,
            Self::TaskCompleted => PersistenceClass::Ephemeral,
            Self::Stop => {
                if data.get("turn_id").is_some() {
                    PersistenceClass::Ephemeral
                } else {
                    PersistenceClass::DiagnosticsOnly
                }
            }
            Self::SessionEnd => {
                if data.as_object().is_some_and(|fields| {
                    fields.keys().any(|key| key != "session_id" && key != "cwd")
                }) {
                    PersistenceClass::Ephemeral
                } else {
                    PersistenceClass::DiagnosticsOnly
                }
            }
            Self::SessionStart | Self::PreToolUse | Self::SubagentStart | Self::SubagentStop => {
                PersistenceClass::Ephemeral
            }
            Self::Notification => PersistenceClass::DiagnosticsOnly,
        }
    }

    fn canonical_data(self, payload: Value) -> Result<Value, String> {
        let Value::Object(payload) = payload else {
            return Err(format!("{} payload must be an object", self.hook_type(),));
        };

        match self {
            Self::SessionStart => canonical_session_start(payload),
            Self::PromptSubmit => canonical_prompt_submit(payload),
            Self::PreToolUse => canonical_pre_tool_use(payload),
            Self::PostToolUse => canonical_post_tool_use(payload),
            Self::PostToolFailure => canonical_post_tool_failure(payload),
            Self::AssistantResult => canonical_assistant_result(payload),
            Self::SubagentStart => canonical_passthrough(
                payload,
                &[
                    "session_id",
                    "turn_id",
                    "cwd",
                    "parent_agent_path",
                    "child_thread_id",
                    "child_agent_path",
                    "child_agent_nickname",
                    "child_agent_role",
                    "task_name",
                ],
            ),
            Self::SubagentStop => canonical_passthrough(
                payload,
                &[
                    "session_id",
                    "cwd",
                    "parent_thread_id",
                    "agent_path",
                    "status",
                    "delivered",
                ],
            ),
            Self::Stop => canonical_passthrough(
                payload,
                &[
                    "session_id",
                    "turn_id",
                    "cwd",
                    "model",
                    "last_assistant_message",
                ],
            ),
            Self::Notification => canonical_passthrough(
                payload,
                &[
                    "session_id",
                    "cwd",
                    "parent_thread_id",
                    "agent_path",
                    "message",
                    "delivered",
                ],
            ),
            Self::TaskCompleted => canonical_passthrough(
                payload,
                &[
                    "session_id",
                    "turn_id",
                    "cwd",
                    "model",
                    "status",
                    "last_assistant_message",
                    "reason",
                ],
            ),
            Self::SessionEnd => canonical_passthrough(
                payload,
                &[
                    "session_id",
                    "cwd",
                    "summary_status",
                    "summary_error",
                    "summary_success",
                ],
            ),
        }
    }
}

pub(crate) fn build_observe_payload(event_name: &str, payload: Value) -> Result<Value, String> {
    let family = HookFamily::from_event_name(event_name)
        .ok_or_else(|| format!("unsupported native agentmemory hook type: {event_name}"))?;
    let source_timestamp = extract_source_timestamp(&payload);
    let data = family.canonical_data(payload)?;
    let session_id = required_string(&data, "session_id")?;
    let cwd = required_string(&data, "cwd")?;
    let project = get_git_repo_root(Path::new(&cwd))
        .unwrap_or_else(|| Path::new(&cwd).to_path_buf())
        .to_string_lossy()
        .into_owned();
    let timestamp = Utc::now().to_rfc3339();
    let event_id = build_event_id(family, &data)?;

    let mut body = json!({
        "sessionId": session_id,
        "hookType": family.hook_type(),
        "project": project,
        "cwd": cwd,
        "timestamp": timestamp,
        "source": NATIVE_OBSERVE_SOURCE,
        "payload_version": NATIVE_OBSERVE_PAYLOAD_VERSION,
        "event_id": event_id,
        "capabilities": NATIVE_OBSERVE_CAPABILITIES,
        "persistence_class": family.persistence_class(&data).as_str(),
        "data": data,
    });

    if let Some(source_timestamp) = source_timestamp
        && source_timestamp != timestamp
    {
        body["source_timestamp"] = Value::String(source_timestamp);
    }

    Ok(body)
}

fn canonical_session_start(payload: Map<String, Value>) -> Result<Value, String> {
    canonical_required_optional(payload, &["session_id", "cwd", "model"], &["source"])
}

fn canonical_prompt_submit(payload: Map<String, Value>) -> Result<Value, String> {
    canonical_required_optional(
        payload,
        &["session_id", "turn_id", "cwd", "model", "prompt"],
        &[],
    )
}

fn canonical_pre_tool_use(payload: Map<String, Value>) -> Result<Value, String> {
    let mut data = canonical_required_optional(
        payload.clone(),
        &[
            "session_id",
            "turn_id",
            "cwd",
            "model",
            "tool_name",
            "tool_use_id",
        ],
        &[],
    )?;
    let tool_name = required_string_from_map(&payload, "tool_name")?;
    let command = payload
        .get("command")
        .ok_or_else(|| "pre_tool_use payload missing `command`".to_string())?;
    data["tool_input"] = normalize_tool_input(&tool_name, command);
    Ok(data)
}

fn canonical_post_tool_use(payload: Map<String, Value>) -> Result<Value, String> {
    let mut data = canonical_pre_tool_use(payload.clone())?;
    let tool_response = payload
        .get("tool_response")
        .ok_or_else(|| "post_tool_use payload missing `tool_response`".to_string())?;
    data["tool_output"] = normalize_tool_output(tool_response);
    Ok(data)
}

fn canonical_post_tool_failure(payload: Map<String, Value>) -> Result<Value, String> {
    let mut data = canonical_pre_tool_use(payload.clone())?;
    let tool_response = payload
        .get("tool_response")
        .ok_or_else(|| "post_tool_failure payload missing `tool_response`".to_string())?;
    data["error"] = normalize_error(tool_response);
    Ok(data)
}

fn canonical_assistant_result(payload: Map<String, Value>) -> Result<Value, String> {
    canonical_required_optional(
        payload,
        &[
            "session_id",
            "turn_id",
            "cwd",
            "model",
            "assistant_text",
            "is_final",
        ],
        &[],
    )
}

fn canonical_passthrough(payload: Map<String, Value>, fields: &[&str]) -> Result<Value, String> {
    let mut data = Map::new();

    for field in fields {
        if let Some(value) = payload.get(*field) {
            data.insert((*field).to_string(), value.clone());
        }
    }

    if !data.contains_key("session_id") {
        return Err("native payload missing `session_id`".to_string());
    }
    if !data.contains_key("cwd") {
        return Err("native payload missing explicit `cwd`".to_string());
    }

    Ok(Value::Object(data))
}

fn canonical_required_optional(
    payload: Map<String, Value>,
    required: &[&str],
    optional: &[&str],
) -> Result<Value, String> {
    let mut data = Map::new();

    for field in required {
        let value = payload
            .get(*field)
            .cloned()
            .ok_or_else(|| format!("native payload missing `{field}`"))?;
        data.insert((*field).to_string(), value);
    }

    for field in optional {
        if let Some(value) = payload.get(*field) {
            data.insert((*field).to_string(), value.clone());
        }
    }

    Ok(Value::Object(data))
}

fn required_string(payload: &Value, key: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("native payload missing non-empty `{key}`"))
}

fn required_string_from_map(payload: &Map<String, Value>, key: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("native payload missing non-empty `{key}`"))
}

fn normalize_tool_input(tool_name: &str, command: &Value) -> Value {
    match command {
        Value::String(command) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(command) {
                return parsed;
            }

            if matches!(tool_name, "Edit" | "Write") {
                return json!({
                    "patch": command,
                    "paths": extract_patch_paths(command),
                });
            }

            json!({ "command": command })
        }
        _ => command.clone(),
    }
}

fn normalize_tool_output(tool_response: &Value) -> Value {
    match tool_response {
        Value::String(text) => {
            serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        _ => tool_response.clone(),
    }
}

fn normalize_error(tool_response: &Value) -> Value {
    match tool_response {
        Value::Object(object) => object
            .get("error")
            .cloned()
            .unwrap_or_else(|| Value::Object(object.clone())),
        _ => tool_response.clone(),
    }
}

fn extract_patch_paths(command: &str) -> Vec<String> {
    let mut paths = Vec::new();

    for prefix in [
        "*** Update File: ",
        "*** Add File: ",
        "*** Delete File: ",
        "*** Move to: ",
    ] {
        for line in command.lines() {
            if let Some(path) = line.strip_prefix(prefix)
                && !paths.iter().any(|existing| existing == path)
            {
                paths.push(path.to_string());
            }
        }
    }

    paths
}

fn extract_source_timestamp(payload: &Value) -> Option<String> {
    payload
        .get("source_timestamp")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            payload
                .get("timestamp")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

fn build_event_id(family: HookFamily, data: &Value) -> Result<String, String> {
    let mut identity = BTreeMap::new();
    identity.insert(
        "hook_type".to_string(),
        Value::String(family.hook_type().to_string()),
    );

    for field in identity_fields(family) {
        if let Some(value) = data.get(field) {
            identity.insert((*field).to_string(), value.clone());
        }
    }

    if !identity.contains_key("session_id") {
        return Err("native payload missing `session_id` for event identity".to_string());
    }

    let identity_json = serde_json::to_string(&identity)
        .map_err(|err| format!("failed to encode observe event identity: {err}"))?;
    Ok(Uuid::new_v5(&Uuid::NAMESPACE_OID, identity_json.as_bytes()).to_string())
}

fn identity_fields(family: HookFamily) -> &'static [&'static str] {
    match family {
        HookFamily::SessionStart => &["session_id"],
        HookFamily::PromptSubmit => &["session_id", "turn_id"],
        HookFamily::PreToolUse | HookFamily::PostToolUse | HookFamily::PostToolFailure => {
            &["session_id", "turn_id", "tool_use_id", "tool_name"]
        }
        HookFamily::AssistantResult => &["session_id", "turn_id", "is_final"],
        HookFamily::SubagentStart => &[
            "session_id",
            "turn_id",
            "child_thread_id",
            "child_agent_path",
        ],
        HookFamily::SubagentStop => &["session_id", "parent_thread_id", "agent_path", "status"],
        HookFamily::Stop => &["session_id", "turn_id"],
        HookFamily::Notification => &["session_id", "parent_thread_id", "agent_path", "message"],
        HookFamily::TaskCompleted => &["session_id", "turn_id", "status"],
        HookFamily::SessionEnd => &["session_id"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn build_observe_payload_normalizes_post_tool_use_contract() {
        let body = build_observe_payload(
            "PostToolUse",
            json!({
                "session_id": "session-1",
                "turn_id": "turn-1",
                "cwd": "/tmp/project",
                "model": "gpt-5",
                "tool_name": "Grep",
                "tool_use_id": "toolu-1",
                "command": "{\"pattern\":\"memory\",\"paths\":[\"src\"]}",
                "tool_response": "match at src/lib.rs",
            }),
        )
        .expect("payload should build");

        assert_eq!(body["hookType"], "post_tool_use");
        assert_eq!(body["source"], "codex-native");
        assert_eq!(body["payload_version"], "1");
        assert_eq!(body["persistence_class"], "persistent");
        assert_eq!(body["capabilities"], json!(NATIVE_OBSERVE_CAPABILITIES));
        assert_eq!(
            body["data"],
            json!({
                "session_id": "session-1",
                "turn_id": "turn-1",
                "cwd": "/tmp/project",
                "model": "gpt-5",
                "tool_name": "Grep",
                "tool_use_id": "toolu-1",
                "tool_input": {
                    "pattern": "memory",
                    "paths": ["src"],
                },
                "tool_output": "match at src/lib.rs",
            }),
        );
        assert!(body.get("event_id").is_some());
    }

    #[test]
    fn build_observe_payload_normalizes_post_tool_failure_contract() {
        let body = build_observe_payload(
            "PostToolUseFailure",
            json!({
                "session_id": "session-1",
                "turn_id": "turn-1",
                "cwd": "/tmp/project",
                "model": "gpt-5",
                "tool_name": "Read",
                "tool_use_id": "toolu-2",
                "command": "{\"path\":\"src/main.rs\"}",
                "tool_response": {
                    "error": "permission denied",
                },
            }),
        )
        .expect("payload should build");

        assert_eq!(body["hookType"], "post_tool_failure");
        assert_eq!(body["data"]["tool_input"], json!({ "path": "src/main.rs" }));
        assert_eq!(body["data"]["error"], json!("permission denied"));
    }

    #[test]
    fn build_observe_payload_extracts_patch_input_for_apply_patch_write_lane() {
        let patch = "*** Begin Patch\n*** Add File: src/new.rs\n+fn main() {}\n*** End Patch";
        let body = build_observe_payload(
            "PostToolUse",
            json!({
                "session_id": "session-1",
                "turn_id": "turn-1",
                "cwd": "/tmp/project",
                "model": "gpt-5",
                "tool_name": "Write",
                "tool_use_id": "toolu-3",
                "command": patch,
                "tool_response": "Success. Updated the following files:\nA src/new.rs",
            }),
        )
        .expect("payload should build");

        assert_eq!(
            body["data"]["tool_input"],
            json!({
                "patch": patch,
                "paths": ["src/new.rs"],
            }),
        );
    }

    #[test]
    fn build_observe_payload_marks_bare_shutdown_events_as_diagnostics_only() {
        let stop = build_observe_payload(
            "Stop",
            json!({
                "session_id": "session-1",
                "cwd": "/tmp/project",
            }),
        )
        .expect("stop payload should build");
        let session_end = build_observe_payload(
            "SessionEnd",
            json!({
                "session_id": "session-1",
                "cwd": "/tmp/project",
            }),
        )
        .expect("session end payload should build");

        assert_eq!(stop["persistence_class"], "diagnostics_only");
        assert_eq!(session_end["persistence_class"], "diagnostics_only");
    }

    #[test]
    fn build_observe_payload_keeps_task_completed_ephemeral() {
        let task_completed = build_observe_payload(
            "TaskCompleted",
            json!({
                "session_id": "session-1",
                "turn_id": "turn-1",
                "cwd": "/tmp/project",
                "model": "gpt-5",
                "status": "completed",
                "last_assistant_message": "done",
            }),
        )
        .expect("task completed payload should build");

        assert_eq!(task_completed["persistence_class"], "ephemeral");
    }

    #[test]
    fn build_observe_payload_requires_explicit_cwd() {
        let err = build_observe_payload(
            "TaskCompleted",
            json!({
                "session_id": "session-1",
                "turn_id": "turn-1",
            }),
        )
        .expect_err("payload without cwd should fail");

        assert_eq!(err, "native payload missing explicit `cwd`");
    }

    #[test]
    fn build_observe_payload_rejects_unknown_hook_types() {
        let err = build_observe_payload(
            "UnknownHook",
            json!({
                "session_id": "session-1",
                "cwd": "/tmp/project",
            }),
        )
        .expect_err("unknown hooks should fail");

        assert_eq!(err, "unsupported native agentmemory hook type: UnknownHook");
    }

    #[test]
    fn build_observe_payload_uses_stable_event_identity() {
        let payload = json!({
            "session_id": "session-1",
            "turn_id": "turn-1",
            "cwd": "/tmp/project",
            "model": "gpt-5",
            "prompt": "show me the payload",
        });
        let first = build_observe_payload("UserPromptSubmit", payload.clone())
            .expect("first payload should build");
        let second = build_observe_payload("UserPromptSubmit", payload)
            .expect("second payload should build");

        assert_eq!(first["event_id"], second["event_id"]);
    }
}
