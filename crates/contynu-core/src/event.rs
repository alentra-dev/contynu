use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{ContynuError, Result};
use crate::ids::{EventId, SessionId, TurnId};

pub const EVENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    System,
    User,
    Assistant,
    Tool,
    Runtime,
    Filesystem,
    Adapter,
}

impl Actor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::Runtime => "runtime",
            Self::Filesystem => "filesystem",
            Self::Adapter => "adapter",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    SessionStarted,
    SessionInterrupted,
    SessionResumed,
    SessionEnded,
    AdapterAttached,
    AdapterDetached,
    TurnStarted,
    TurnCompleted,
    TurnFailed,
    TurnCancelled,
    MessageInput,
    MessageOutput,
    MessageChunk,
    MessageRedaction,
    ToolCall,
    ToolResult,
    ToolStream,
    ToolError,
    StdinCaptured,
    StdoutCaptured,
    StderrCaptured,
    ProcessStarted,
    ProcessExited,
    FileObserved,
    FileSnapshot,
    FileDiff,
    FileDeleted,
    WorkspaceScanCompleted,
    ArtifactRegistered,
    ArtifactMaterialized,
    ArtifactRead,
    ArtifactDeleted,
    CheckpointCreated,
    RehydrationPacketCreated,
    MemoryObjectDerived,
    MemoryObjectSuperseded,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStarted => "session_started",
            Self::SessionInterrupted => "session_interrupted",
            Self::SessionResumed => "session_resumed",
            Self::SessionEnded => "session_ended",
            Self::AdapterAttached => "adapter_attached",
            Self::AdapterDetached => "adapter_detached",
            Self::TurnStarted => "turn_started",
            Self::TurnCompleted => "turn_completed",
            Self::TurnFailed => "turn_failed",
            Self::TurnCancelled => "turn_cancelled",
            Self::MessageInput => "message_input",
            Self::MessageOutput => "message_output",
            Self::MessageChunk => "message_chunk",
            Self::MessageRedaction => "message_redaction",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::ToolStream => "tool_stream",
            Self::ToolError => "tool_error",
            Self::StdinCaptured => "stdin_captured",
            Self::StdoutCaptured => "stdout_captured",
            Self::StderrCaptured => "stderr_captured",
            Self::ProcessStarted => "process_started",
            Self::ProcessExited => "process_exited",
            Self::FileObserved => "file_observed",
            Self::FileSnapshot => "file_snapshot",
            Self::FileDiff => "file_diff",
            Self::FileDeleted => "file_deleted",
            Self::WorkspaceScanCompleted => "workspace_scan_completed",
            Self::ArtifactRegistered => "artifact_registered",
            Self::ArtifactMaterialized => "artifact_materialized",
            Self::ArtifactRead => "artifact_read",
            Self::ArtifactDeleted => "artifact_deleted",
            Self::CheckpointCreated => "checkpoint_created",
            Self::RehydrationPacketCreated => "rehydration_packet_created",
            Self::MemoryObjectDerived => "memory_object_derived",
            Self::MemoryObjectSuperseded => "memory_object_superseded",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDraft {
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub ts: DateTime<Utc>,
    pub actor: Actor,
    pub event_type: EventType,
    pub payload_version: u32,
    pub payload: Value,
    pub parent_event_id: Option<EventId>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<EventId>,
    pub tags: Vec<String>,
}

impl EventDraft {
    pub fn new(
        session_id: SessionId,
        turn_id: Option<TurnId>,
        actor: Actor,
        event_type: EventType,
        payload: Value,
    ) -> Self {
        Self {
            session_id,
            turn_id,
            ts: Utc::now(),
            actor,
            event_type,
            payload_version: 1,
            payload,
            parent_event_id: None,
            correlation_id: None,
            causation_id: None,
            tags: Vec::new(),
        }
    }

    pub fn finalize(self, seq: u64) -> Result<EventEnvelope> {
        let mut event = EventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(),
            session_id: self.session_id,
            turn_id: self.turn_id,
            seq,
            ts: self.ts,
            actor: self.actor,
            event_type: self.event_type,
            payload_version: self.payload_version,
            payload: self.payload,
            checksum: String::new(),
            parent_event_id: self.parent_event_id,
            correlation_id: self.correlation_id,
            causation_id: self.causation_id,
            tags: self.tags,
        };
        event.checksum = event.compute_checksum()?;
        event.validate()?;
        Ok(event)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub schema_version: u32,
    pub event_id: EventId,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub actor: Actor,
    pub event_type: EventType,
    pub payload_version: u32,
    pub payload: Value,
    pub checksum: String,
    pub parent_event_id: Option<EventId>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<EventId>,
    pub tags: Vec<String>,
}

impl EventEnvelope {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != EVENT_SCHEMA_VERSION {
            return Err(ContynuError::Validation(format!(
                "unsupported schema_version {}",
                self.schema_version
            )));
        }
        if self.seq == 0 {
            return Err(ContynuError::Validation("seq must be >= 1".into()));
        }
        if !self.payload.is_object() {
            return Err(ContynuError::Validation(
                "payload must be a JSON object".into(),
            ));
        }
        let expected = self.compute_checksum()?;
        if self.checksum != expected {
            return Err(ContynuError::ChecksumMismatch {
                event_id: self.event_id.to_string(),
            });
        }
        Ok(())
    }

    pub fn compute_checksum(&self) -> Result<String> {
        let value = serde_json::to_value(self)?;
        let object = value.as_object().ok_or_else(|| {
            ContynuError::Validation("event did not serialize to a JSON object".into())
        })?;
        let mut object = object.clone();
        object.remove("checksum");
        let canonical = canonicalize_object(&object);
        Ok(format!("sha256:{}", sha256_hex(canonical.as_bytes())))
    }

    pub fn to_json_line(&self) -> Result<String> {
        self.validate()?;
        Ok(serde_json::to_string(self)?)
    }

    pub fn summary_text(&self) -> Option<String> {
        let payload = self.payload.as_object()?;
        match self.event_type {
            EventType::MessageInput | EventType::MessageOutput | EventType::MessageChunk => {
                extract_content_text(payload)
            }
            EventType::StdoutCaptured | EventType::StderrCaptured | EventType::StdinCaptured => {
                payload.get("text")?.as_str().map(str::to_owned)
            }
            EventType::ToolCall => payload.get("tool_name")?.as_str().map(str::to_owned),
            EventType::ToolResult => payload.get("status")?.as_str().map(str::to_owned),
            _ => None,
        }
    }
}

pub fn canonicalize_value(value: &Value) -> String {
    match value {
        Value::Object(map) => canonicalize_object(map),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(canonicalize_value)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{inner}]")
        }
        _ => serde_json::to_string(value).expect("primitive values serialize"),
    }
}

pub fn canonicalize_object(map: &Map<String, Value>) -> String {
    let ordered: BTreeMap<&String, &Value> = map.iter().collect();
    let mut out = String::from("{");
    for (index, (key, value)) in ordered.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(key).expect("keys serialize"));
        out.push(':');
        out.push_str(&canonicalize_value(value));
    }
    out.push('}');
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn extract_content_text(payload: &Map<String, Value>) -> Option<String> {
    let items = payload.get("content")?.as_array()?;
    let text = items
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Actor, EventDraft, EventType};
    use crate::ids::SessionId;

    #[test]
    fn finalized_events_have_stable_checksums() {
        let draft = EventDraft::new(
            SessionId::new(),
            None,
            Actor::Runtime,
            EventType::SessionStarted,
            json!({"cwd": "/tmp"}),
        );

        let event = draft.finalize(1).unwrap();
        assert_eq!(event.compute_checksum().unwrap(), event.checksum);
    }
}
