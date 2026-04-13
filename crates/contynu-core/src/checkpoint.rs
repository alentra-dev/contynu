use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::blobs::BlobStore;
use crate::error::Result;
use crate::ids::{CheckpointId, MemoryId, ProjectId, SessionId};
use crate::state::StatePaths;
use crate::store::{CheckpointRecord, MemoryObject, MemoryObjectKind, MetadataStore};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RehydrationArtifact {
    pub path: String,
    pub kind: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryProvenance {
    pub memory_id: String,
    pub kind: String,
    pub source_model: Option<String>,
    pub importance: f64,
}

pub struct PacketBudget {
    pub max_total_tokens: usize,
    pub max_per_category: usize,
    pub min_per_category: usize,
}

impl Default for PacketBudget {
    fn default() -> Self {
        Self {
            max_total_tokens: 4000,
            max_per_category: 20,
            min_per_category: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RehydrationPacket {
    pub schema_version: u32,
    #[serde(default)]
    pub project_identity: String,
    #[serde(default)]
    pub compact_brief: String,
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
    #[serde(default)]
    pub memory_provenance: Vec<MemoryProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointManifest {
    pub checkpoint_id: CheckpointId,
    pub project_id: ProjectId,
    pub created_at: DateTime<Utc>,
    pub reason: String,
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

        let manifest = CheckpointManifest {
            checkpoint_id: checkpoint_id.clone(),
            project_id: session_id.clone(),
            created_at: Utc::now(),
            reason: reason.to_string(),
            rehydration_blob_sha: Some(packet_blob.sha256.clone()),
            checkpoint_dir: path_string(&checkpoint_dir),
        };
        fs::write(
            checkpoint_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest)?,
        )?;

        self.store.register_checkpoint(
            &CheckpointRecord {
                checkpoint_id: manifest.checkpoint_id.clone(),
                session_id: manifest.project_id.clone(),
                reason: manifest.reason.clone(),
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
        self.build_packet_with_budget(session_id, target_model, &PacketBudget::default())
    }

    pub fn build_packet_with_budget(
        &self,
        session_id: &SessionId,
        target_model: Option<String>,
        budget: &PacketBudget,
    ) -> Result<RehydrationPacket> {
        let memory = self.store.list_active_memories(session_id, None)?;
        let recent_prompts = self.store.list_recent_prompts(session_id, 5)?;

        // Mission: latest user prompt
        let mission = recent_prompts
            .first()
            .map(|p| p.verbatim.clone())
            .unwrap_or_else(|| "Continue the session faithfully from canonical state.".into());

        let mut provenance = Vec::new();
        let mut accessed_ids = Vec::new();

        let (stable_facts, fact_ids) =
            select_memories(&memory, MemoryObjectKind::Fact, budget);
        let (constraints, constraint_ids) =
            select_memories(&memory, MemoryObjectKind::Constraint, budget);
        let (decisions, decision_ids) =
            select_memories(&memory, MemoryObjectKind::Decision, budget);
        let (open_loops, todo_ids) =
            select_memories(&memory, MemoryObjectKind::Todo, budget);

        for ids in [&fact_ids, &constraint_ids, &decision_ids, &todo_ids] {
            accessed_ids.extend(ids.iter().cloned());
        }

        // Build provenance from the selected memories
        for mem in &memory {
            if accessed_ids.contains(&mem.memory_id) {
                provenance.push(MemoryProvenance {
                    memory_id: mem.memory_id.to_string(),
                    kind: mem.kind.as_str().to_string(),
                    source_model: mem.source_model.clone(),
                    importance: mem.importance,
                });
            }
        }

        // Track access for included memories
        let _ = self.store.increment_memory_access(&accessed_ids);

        // Recent verbatim context from recorded prompts
        let recent_verbatim_context: Vec<String> = recent_prompts
            .iter()
            .rev()
            .map(|p| {
                if let Some(ref interp) = p.interpretation {
                    format!("User: {} (interpreted as: {})", p.verbatim, interp)
                } else {
                    format!("User: {}", p.verbatim)
                }
            })
            .collect();

        let current_state = if let Some(prompt) = recent_prompts.first() {
            format!("The latest user request was: {}", prompt.verbatim)
        } else {
            format!(
                "Session has {} active memory objects.",
                memory.len(),
            )
        };

        // L0: Project identity
        let session_info = self.store.get_session(session_id)?;
        let project_identity = if let Some(ref session) = session_info {
            format!(
                "Project {} | {} | {} memories",
                session_id,
                session.cli_name.as_deref().unwrap_or("unknown"),
                memory.len(),
            )
        } else {
            format!("Project {}", session_id)
        };

        // L1: Compact brief — top 15 memories, one line each
        let mut brief_lines = Vec::new();
        let mut brief_chars = 0usize;
        let max_brief_chars = 2000;
        for m in &memory {
            let text = m.text.trim();
            let kind_abbrev = match m.kind {
                MemoryObjectKind::Fact => "F",
                MemoryObjectKind::Decision => "D",
                MemoryObjectKind::Constraint => "C",
                MemoryObjectKind::Todo => "T",
                MemoryObjectKind::UserFact => "U",
                MemoryObjectKind::ProjectKnowledge => "P",
            };
            let line = format!("{}: {}", kind_abbrev, text);
            if brief_chars + line.len() > max_brief_chars {
                break;
            }
            brief_lines.push(line.clone());
            brief_chars += line.len();
            if brief_lines.len() >= 15 {
                break;
            }
        }
        let compact_brief = brief_lines.join("\n");

        // Retrieval guidance — tell models how to use Contynu MCP tools
        let retrieval_guidance = vec![
            "Use the Contynu MCP tools to write memories at each stop point.".into(),
            "Call write_memory for facts, decisions, constraints worth recalling later.".into(),
            "Call record_prompt with the user's verbatim input at each stop point.".into(),
            "Call search_memory to check for existing knowledge before duplicating.".into(),
            "Call update_memory to correct or refine existing memories instead of creating duplicates.".into(),
        ];

        Ok(RehydrationPacket {
            schema_version: 3,
            project_id: session_id.clone(),
            project_identity,
            compact_brief,
            target_model,
            mission,
            stable_facts,
            constraints,
            decisions,
            current_state,
            open_loops,
            relevant_artifacts: Vec::new(),
            relevant_files: Vec::new(),
            recent_verbatim_context,
            retrieval_guidance,
            memory_provenance: provenance,
        })
    }
}

/// Backward-compatible wrapper: renders using Markdown format.
pub fn render_rehydration_prompt(packet: &RehydrationPacket, adapter_name: &str) -> String {
    crate::rendering::render_rehydration(
        packet,
        crate::rendering::PromptFormat::Markdown,
        adapter_name,
    )
}

/// Backward-compatible wrapper: renders using StructuredText format.
pub fn render_launcher_prompt(packet: &RehydrationPacket) -> String {
    crate::rendering::render_launcher(packet, crate::rendering::PromptFormat::StructuredText)
}

/// Selects the top memories by importance within the budget.
/// Returns (selected texts, selected memory IDs).
fn select_memories(
    memories: &[MemoryObject],
    kind: MemoryObjectKind,
    budget: &PacketBudget,
) -> (Vec<String>, Vec<MemoryId>) {
    // Memories are already sorted by importance DESC from the store query
    let filtered: Vec<&MemoryObject> = memories
        .iter()
        .filter(|m| m.kind == kind)
        .collect();

    let limit = budget.max_per_category.max(budget.min_per_category);
    let token_budget = budget.max_total_tokens / 4;
    let mut texts = Vec::new();
    let mut ids = Vec::new();
    let mut token_estimate = 0usize;

    for m in filtered.iter().take(limit) {
        let word_count = m.text.split_whitespace().count();
        let tokens = (word_count as f64 * 1.3) as usize;
        if token_estimate + tokens > token_budget && texts.len() >= budget.min_per_category {
            break;
        }
        // Full text, no truncation
        texts.push(m.text.clone());
        ids.push(m.memory_id.clone());
        token_estimate += tokens;
    }

    (texts, ids)
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{render_launcher_prompt, CheckpointManager, RehydrationPacket};
    use crate::blobs::BlobStore;
    use crate::ids::{MemoryId, ProjectId, SessionId};
    use crate::state::StatePaths;
    use crate::store::{
        MemoryObject, MemoryObjectKind, MemoryScope, MetadataStore, PromptRecord, SessionRecord,
    };

    fn prompt_packet() -> RehydrationPacket {
        RehydrationPacket {
            schema_version: 3,
            project_identity: String::new(),
            compact_brief: String::new(),
            project_id: ProjectId::parse("prj_019d503680a475a3ae465200a90cd4fa").unwrap(),
            target_model: None,
            mission: "Continue the session faithfully from canonical state.".into(),
            stable_facts: vec![
                "Frank secret santa is Dun".into(),
            ],
            constraints: Vec::new(),
            decisions: vec!["Keep this in continuity memory".into()],
            current_state:
                "The latest user request was: what is the name of Frank's secret santa?"
                    .into(),
            open_loops: vec!["Confirm the next model can recall Frank's secret santa.".into()],
            relevant_artifacts: Vec::new(),
            relevant_files: Vec::new(),
            recent_verbatim_context: vec![
                "User: what is the name of Frank's secret santa?".into(),
            ],
            retrieval_guidance: Vec::new(),
            memory_provenance: Vec::new(),
        }
    }

    #[test]
    fn launcher_prompt_filters_operational_noise() {
        let prompt = render_launcher_prompt(&prompt_packet());
        assert!(prompt.contains("Frank secret santa is Dun"));
        assert!(prompt.contains("Decisions: Keep this in continuity memory"));
    }

    #[test]
    fn launcher_prompt_keeps_meaningful_current_state() {
        let mut packet = prompt_packet();
        packet.current_state =
            "The user corrected the spelling of Frank's name and wants that remembered.".into();
        let prompt = render_launcher_prompt(&packet);
        assert!(prompt.contains(
            "Current focus: The user corrected the spelling of Frank's name and wants that remembered."
        ));
    }

    #[test]
    fn checkpoint_generation_produces_packet() {
        let dir = tempdir().unwrap();
        let state = StatePaths::new(dir.path().join(".contynu"));
        state.ensure_layout().unwrap();

        let store = MetadataStore::open(state.sqlite_db()).unwrap();
        let blobs = BlobStore::new(state.blobs_root());
        let session_id = SessionId::new();

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

        store
            .insert_prompt(&PromptRecord {
                prompt_id: "pmt_test1".into(),
                session_id: session_id.clone(),
                verbatim: "Fix the authentication bug".into(),
                interpretation: None,
                interpretation_confidence: None,
                source_model: None,
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        store
            .insert_memory_object(&MemoryObject {
                memory_id: MemoryId::new(),
                session_id: session_id.clone(),
                kind: MemoryObjectKind::Decision,
                scope: MemoryScope::Project,
                status: "active".into(),
                text: "Use SQLite as the metadata store.".into(),
                importance: 0.85,
                reason: None,
                source_model: None,
                superseded_by: None,
                created_at: chrono::Utc::now(),
                updated_at: None,
                access_count: 0,
                last_accessed_at: None,
            })
            .unwrap();

        let manager = CheckpointManager::new(&state, &store, &blobs);
        let (manifest, packet) = manager
            .create_checkpoint(&session_id, "test", None)
            .unwrap();

        assert_eq!(manifest.project_id, session_id);
        assert!(packet.mission.contains("Fix the authentication bug"));
    }

    #[test]
    fn build_packet_uses_recent_prompts() {
        let dir = tempdir().unwrap();
        let state = StatePaths::new(dir.path().join(".contynu"));
        state.ensure_layout().unwrap();

        let store = MetadataStore::open(state.sqlite_db()).unwrap();
        let blobs = BlobStore::new(state.blobs_root());
        let session_id = SessionId::new();

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

        for (verbatim, interp) in [
            ("why is the sky blue?", None),
            ("what did you just say about the sky?", Some("Recalling previous answer about Rayleigh scattering")),
        ] {
            store
                .insert_prompt(&PromptRecord {
                    prompt_id: format!("pmt_{}", uuid::Uuid::now_v7().simple()),
                    session_id: session_id.clone(),
                    verbatim: verbatim.into(),
                    interpretation: interp.map(String::from),
                    interpretation_confidence: interp.map(|_| 0.9),
                    source_model: None,
                    created_at: chrono::Utc::now(),
                })
                .unwrap();
        }

        let manager = CheckpointManager::new(&state, &store, &blobs);
        let packet = manager.build_packet(&session_id, None).unwrap();

        assert_eq!(packet.mission, "what did you just say about the sky?");
        assert!(packet
            .recent_verbatim_context
            .iter()
            .any(|line| line.contains("why is the sky blue?")));
    }
}
