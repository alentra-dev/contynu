use crate::ids::{EventId, SessionId, TurnId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub schema_version: u32,
    pub event_id: EventId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub actor: Actor,
    pub event_type: String,
    pub payload_version: u32,
    pub payload: Value,
    pub checksum: Option<String>,
    pub parent_event_id: Option<EventId>,
    pub correlation_id: Option<String>,
    pub causation_id: Option<EventId>,
    pub tags: Vec<String>,
}

impl EventEnvelope {
    pub fn new(session_id: SessionId, turn_id: TurnId, actor: Actor, event_type: impl Into<String>, payload: Value) -> Self {
        Self {
            schema_version: 1,
            event_id: EventId::new(),
            session_id,
            turn_id,
            seq: 0,
            ts: Utc::now(),
            actor,
            event_type: event_type.into(),
            payload_version: 1,
            payload,
            checksum: None,
            parent_event_id: None,
            correlation_id: None,
            causation_id: None,
            tags: Vec::new(),
        }
    }
}
