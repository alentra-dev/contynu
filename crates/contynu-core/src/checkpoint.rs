use crate::ids::{CheckpointId, SessionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointManifest {
    pub checkpoint_id: CheckpointId,
    pub session_id: SessionId,
    pub created_at: DateTime<Utc>,
    pub reason: String,
    pub last_seq: u64,
    pub rehydration_blob_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RehydrationPacket {
    pub mission: String,
    pub stable_facts: Vec<String>,
    pub constraints: Vec<String>,
    pub decisions: Vec<String>,
    pub current_state: String,
    pub open_loops: Vec<String>,
    pub relevant_artifacts: Vec<String>,
    pub recent_context: Vec<String>,
}
