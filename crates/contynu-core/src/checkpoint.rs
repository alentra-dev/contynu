use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::blobs::BlobStore;
use crate::error::Result;
use crate::event::{Actor, EventDraft, EventType};
use crate::ids::{CheckpointId, ProjectId, SessionId};
use crate::journal::Journal;
use crate::state::StatePaths;
use crate::store::{CheckpointRecord, MemoryObjectKind, MetadataStore};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RehydrationArtifact {
    pub path: String,
    pub kind: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RehydrationPacket {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub target_model: Option<String>,
    pub mission: String,
    pub stable_facts: Vec<String>,
    pub constraints: Vec<String>,
    pub decisions: Vec<String>,
    pub current_state: String,
    pub open_loops: Vec<String>,
    pub relevant_artifacts: Vec<RehydrationArtifact>,
    pub relevant_files: Vec<String>,
    pub recent_verbatim_context: Vec<String>,
    pub retrieval_guidance: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointManifest {
    pub checkpoint_id: CheckpointId,
    pub project_id: ProjectId,
    pub created_at: DateTime<Utc>,
    pub reason: String,
    pub last_seq: u64,
    pub rehydration_blob_sha: Option<String>,
    pub checkpoint_dir: String,
}

pub struct CheckpointManager<'a> {
    state_paths: &'a StatePaths,
    store: &'a MetadataStore,
    blob_store: &'a BlobStore,
}

impl<'a> CheckpointManager<'a> {
    pub fn new(
        state_paths: &'a StatePaths,
        store: &'a MetadataStore,
        blob_store: &'a BlobStore,
    ) -> Self {
        Self {
            state_paths,
            store,
            blob_store,
        }
    }

    pub fn create_checkpoint(
        &self,
        journal: &Journal,
        session_id: &SessionId,
        reason: &str,
        target_model: Option<String>,
    ) -> Result<(CheckpointManifest, RehydrationPacket)> {
        let packet = self.build_packet(session_id, target_model)?;
        let packet_json = serde_json::to_string_pretty(&packet)?;
        let packet_blob = self.blob_store.put_text(&packet_json)?;
        self.store
            .register_blob(&packet_blob, Some("application/json"))?;

        let checkpoint_id = CheckpointId::new();
        let checkpoint_dir = self.state_paths.checkpoint_dir(session_id, &checkpoint_id);
        fs::create_dir_all(&checkpoint_dir)?;
        fs::write(checkpoint_dir.join("rehydration.json"), &packet_json)?;

        let last_seq = self
            .store
            .list_events_for_session(session_id)?
            .last()
            .map(|event| event.seq)
            .unwrap_or(0);
        let manifest = CheckpointManifest {
            checkpoint_id: checkpoint_id.clone(),
            project_id: session_id.clone(),
            created_at: Utc::now(),
            reason: reason.to_string(),
            last_seq,
            rehydration_blob_sha: Some(packet_blob.sha256.clone()),
            checkpoint_dir: path_string(&checkpoint_dir),
        };
        fs::write(
            checkpoint_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest)?,
        )?;

        let (event, append) = journal.append(EventDraft::new(
            session_id.clone(),
            None,
            Actor::System,
            EventType::CheckpointCreated,
            json!({
                "checkpoint_id": checkpoint_id,
                "reason": reason,
                "rehydration_blob_sha": packet_blob.sha256,
            }),
        ))?;
        self.store
            .record_event(&event, &journal.path().display().to_string(), append)?;
        self.store.register_checkpoint(
            &CheckpointRecord {
                checkpoint_id: manifest.checkpoint_id.clone(),
                session_id: manifest.project_id.clone(),
                source_event_id: event.event_id.clone(),
                reason: manifest.reason.clone(),
                last_seq: manifest.last_seq,
                rehydration_sha256: manifest.rehydration_blob_sha.clone(),
                created_at: manifest.created_at,
            },
            &manifest,
        )?;

        Ok((manifest, packet))
    }

    pub fn build_packet(
        &self,
        session_id: &SessionId,
        target_model: Option<String>,
    ) -> Result<RehydrationPacket> {
        let events = self.store.list_events_for_session(session_id)?;
        let memory = self.store.list_memory_objects(session_id, None)?;
        let artifacts = self.store.list_artifacts(Some(session_id))?;

        let mission = events
            .iter()
            .find_map(|event| {
                if event.event_type == "message_input" {
                    event
                        .payload_json
                        .get("content")
                        .and_then(|value| value.as_array())
                        .and_then(|items| items.first())
                        .and_then(|item| item.get("text"))
                        .and_then(|text| text.as_str())
                        .map(str::to_owned)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "Continue the session faithfully from canonical state.".into());

        let stable_facts = memory_texts(&memory, MemoryObjectKind::Fact);
        let constraints = memory_texts(&memory, MemoryObjectKind::Constraint);
        let decisions = memory_texts(&memory, MemoryObjectKind::Decision);
        let open_loops = memory_texts(&memory, MemoryObjectKind::Todo);
        let relevant_files = Vec::new();
        let recent_verbatim_context = events
            .iter()
            .rev()
            .filter_map(|event| {
                if event.event_type == "message_input"
                    || event.event_type == "message_output"
                    || event.event_type == "stdout_captured"
                    || event.event_type == "stderr_captured"
                {
                    Some(event.payload_json.to_string())
                } else {
                    None
                }
            })
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        let current_state = if let Some(summary) = memory
            .iter()
            .rev()
            .find(|item| item.kind == MemoryObjectKind::Summary)
        {
            summary.text.clone()
        } else {
            format!(
                "Session has {} canonical events, {} memory objects, and {} tracked artifacts.",
                events.len(),
                memory.len(),
                artifacts.len()
            )
        };

        Ok(RehydrationPacket {
            schema_version: 1,
            project_id: session_id.clone(),
            target_model,
            mission,
            stable_facts,
            constraints,
            decisions,
            current_state,
            open_loops,
            relevant_artifacts: artifacts
                .into_iter()
                .map(|artifact| RehydrationArtifact {
                    path: artifact.path.unwrap_or_else(|| "<unknown>".into()),
                    kind: artifact.kind,
                    sha256: artifact.sha256,
                })
                .collect(),
            relevant_files,
            recent_verbatim_context,
            retrieval_guidance: vec![
                "Use the journal for exact replay when precision matters.".into(),
                "Use structured memory objects for durable facts, constraints, and decisions.".into(),
                "Prefer artifacts and tracked files over regenerated summaries when recovering work.".into(),
            ],
        })
    }
}

fn memory_texts(memory: &[crate::store::MemoryObject], kind: MemoryObjectKind) -> Vec<String> {
    memory
        .iter()
        .filter(|item| item.kind == kind)
        .map(|item| item.text.clone())
        .collect()
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::CheckpointManager;
    use crate::blobs::BlobStore;
    use crate::event::{Actor, EventDraft, EventType};
    use crate::ids::{MemoryId, SessionId};
    use crate::journal::Journal;
    use crate::state::StatePaths;
    use crate::store::{MemoryObject, MemoryObjectKind, MetadataStore, SessionRecord};

    #[test]
    fn checkpoint_generation_produces_packet() {
        let dir = tempdir().unwrap();
        let state = StatePaths::new(dir.path().join(".contynu"));
        state.ensure_layout().unwrap();

        let store = MetadataStore::open(state.sqlite_db()).unwrap();
        let blobs = BlobStore::new(state.blobs_root());
        let session_id = SessionId::new();
        let journal = Journal::open(state.journal_path_for_session(&session_id)).unwrap();

        store
            .register_session(&SessionRecord {
                session_id: session_id.clone(),
                project_id: None,
                status: "started".into(),
                cli_name: None,
                cli_version: None,
                model_name: None,
                cwd: None,
                repo_root: None,
                host_fingerprint: None,
                started_at: chrono::Utc::now(),
                ended_at: None,
            })
            .unwrap();
        let (event, append) = journal
            .append(EventDraft::new(
                session_id.clone(),
                None,
                Actor::User,
                EventType::MessageInput,
                json!({"content": [{"type": "text", "text": "Fix the journal"}]}),
            ))
            .unwrap();
        store
            .record_event(&event, &journal.path().display().to_string(), append)
            .unwrap();
        store
            .insert_memory_object(&MemoryObject {
                memory_id: MemoryId::new(),
                session_id: session_id.clone(),
                kind: MemoryObjectKind::Decision,
                status: "active".into(),
                text: "Keep JSONL as canonical truth.".into(),
                confidence: Some(1.0),
                source_event_ids: vec![event.event_id.clone()],
                created_at: chrono::Utc::now(),
                superseded_by: None,
            })
            .unwrap();

        let manager = CheckpointManager::new(&state, &store, &blobs);
        let (manifest, packet) = manager
            .create_checkpoint(&journal, &session_id, "test", None)
            .unwrap();

        assert_eq!(manifest.project_id, session_id);
        assert!(packet.mission.contains("Fix the journal"));
    }
}
