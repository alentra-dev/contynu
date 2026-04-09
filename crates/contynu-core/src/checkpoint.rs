use std::collections::VecDeque;
use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::blobs::BlobStore;
use crate::error::Result;
use crate::event::{Actor, EventDraft, EventType};
use crate::ids::{CheckpointId, MemoryId, ProjectId, SessionId};
use crate::journal::Journal;
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
    pub source_adapter: Option<String>,
    pub source_model: Option<String>,
    pub importance: f64,
}

pub struct PacketBudget {
    pub max_total_tokens: usize,
    pub max_per_category: usize,
    pub min_per_category: usize,
    pub dialogue_turns: usize,
}

impl Default for PacketBudget {
    fn default() -> Self {
        Self {
            max_total_tokens: 4000,
            max_per_category: 20,
            min_per_category: 2,
            dialogue_turns: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RehydrationPacket {
    pub schema_version: u32,
    /// L0: One-line project identity (~50 tokens, always loaded)
    #[serde(default)]
    pub project_identity: String,
    /// L1: Compact brief of top memories (~500 tokens, always loaded)
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
    pub last_seq: u64,
    pub rehydration_blob_sha: Option<String>,
    pub checkpoint_dir: String,
}

pub struct CheckpointManager<'a> {
    state_paths: &'a StatePaths,
    store: &'a MetadataStore,
    blob_store: &'a BlobStore,
}

#[derive(Debug, Clone)]
struct DialogueTurn {
    prompt: String,
    response: String,
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
        self.build_packet_with_budget(session_id, target_model, &PacketBudget::default())
    }

    pub fn build_packet_with_budget(
        &self,
        session_id: &SessionId,
        target_model: Option<String>,
        budget: &PacketBudget,
    ) -> Result<RehydrationPacket> {
        let events = self.store.list_events_for_session(session_id)?;
        let memory = self.store.list_active_memory_objects(session_id, None)?;
        let artifacts = self.store.list_artifacts(Some(session_id))?;
        let dialogue_turns = extract_dialogue_turns(&events);
        let latest_dialogue = dialogue_turns.last();

        let mission = latest_dialogue
            .map(|dialogue| dialogue.prompt.clone())
            .or_else(|| {
                events.iter().find_map(|event| {
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
            })
            .unwrap_or_else(|| "Continue the session faithfully from canonical state.".into());

        let mut provenance = Vec::new();
        let mut accessed_ids = Vec::new();

        let (stable_facts, fact_ids) =
            score_and_select(&memory, MemoryObjectKind::Fact, budget);
        let (constraints, constraint_ids) =
            score_and_select(&memory, MemoryObjectKind::Constraint, budget);
        let (decisions, decision_ids) =
            score_and_select(&memory, MemoryObjectKind::Decision, budget);
        let (open_loops, todo_ids) =
            score_and_select(&memory, MemoryObjectKind::Todo, budget);

        for ids in [&fact_ids, &constraint_ids, &decision_ids, &todo_ids] {
            accessed_ids.extend(ids.iter().cloned());
        }

        // Build provenance from the selected memories
        for mem in &memory {
            if accessed_ids.contains(&mem.memory_id) {
                provenance.push(MemoryProvenance {
                    memory_id: mem.memory_id.to_string(),
                    kind: mem.kind.as_str().to_string(),
                    source_adapter: mem.source_adapter.clone(),
                    source_model: mem.source_model.clone(),
                    importance: mem.importance,
                });
            }
        }

        // Track access for included memories
        let _ = self.store.increment_memory_access(&accessed_ids);

        let relevant_files = Vec::new();
        let recent_verbatim_context = if !dialogue_turns.is_empty() {
            dialogue_turns
                .iter()
                .rev()
                .take(budget.dialogue_turns)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .flat_map(|dialogue| {
                    [
                        format!("User: {}", dialogue.prompt),
                        format!("Assistant: {}", dialogue.response),
                    ]
                })
                .collect::<Vec<_>>()
        } else {
            events
                .iter()
                .rev()
                .filter_map(|event| {
                    if event.event_type == "message_input"
                        || event.event_type == "message_output"
                        || event.event_type == "stdout_captured"
                        || event.event_type == "stderr_captured"
                    {
                        extract_event_summary_text(event)
                            .map(|text| one_line(&text))
                            .filter(|text| !text.is_empty())
                    } else {
                        None
                    }
                })
                .take(budget.dialogue_turns * 2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
        };
        let current_state = if let Some(dialogue) = latest_dialogue {
            format!(
                "The latest user request was: {}. The latest assistant response was: {}",
                dialogue.prompt, dialogue.response
            )
        } else if let Some(summary) = memory
            .iter()
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

        // L0: Project identity — one-line summary (~50 tokens)
        let session_info = self.store.get_session(session_id)?;
        let project_identity = if let Some(ref session) = session_info {
            format!(
                "Project {} | {} | {} memories | {} turns",
                session_id,
                session.cli_name.as_deref().unwrap_or("unknown"),
                memory.len(),
                self.store.list_turns_for_session(session_id)?.len()
            )
        } else {
            format!("Project {}", session_id)
        };

        // L1: Compact brief — top 15 memories, one line each (~500 tokens)
        let mut brief_lines = Vec::new();
        let mut brief_chars = 0usize;
        let max_brief_chars = 2000;
        // Take from all categories, sorted by importance (already sorted)
        for m in &memory {
            if m.kind == MemoryObjectKind::Summary {
                continue;
            }
            // Compress: strip common prefixes, bold markers, truncate
            let text = m.text
                .replace("LLM response: ", "")
                .replace("**", "")
                .replace("Decision: ", "")
                .replace("Fact: ", "")
                .replace("Constraint: ", "")
                .replace("Todo: ", "");
            let text = text.trim();
            let truncated = if text.len() > 100 { &text[..100] } else { text };
            let kind_abbrev = match m.kind {
                MemoryObjectKind::Fact => "F",
                MemoryObjectKind::Decision => "D",
                MemoryObjectKind::Constraint => "C",
                MemoryObjectKind::Todo => "T",
                _ => "•",
            };
            let line = format!("{}: {}", kind_abbrev, truncated);
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

        let max_artifacts = budget.max_per_category;
        Ok(RehydrationPacket {
            schema_version: 2,
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
            relevant_artifacts: artifacts
                .into_iter()
                .rev()
                .take(max_artifacts)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
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
            memory_provenance: provenance,
        })
    }
}

/// Backward-compatible wrapper: renders using StructuredText format.
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

/// Scores memories by importance, recency, and confidence, then selects the top items
/// within the budget. Returns (selected texts, selected memory IDs).
fn score_and_select(
    memories: &[MemoryObject],
    kind: MemoryObjectKind,
    budget: &PacketBudget,
) -> (Vec<String>, Vec<MemoryId>) {
    let now = Utc::now();
    let mut scored: Vec<(f64, &MemoryObject)> = memories
        .iter()
        .filter(|m| m.kind == kind)
        .map(|m| {
            let days_old = (now - m.created_at).num_hours().max(0) as f64 / 24.0;
            let recency = 1.0 / (1.0 + days_old * 0.1);
            let confidence = m.confidence.unwrap_or(0.5);
            let score = m.importance * 0.5 + recency * 0.3 + confidence * 0.2;
            (score, m)
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let limit = budget.max_per_category.max(budget.min_per_category);
    let selected: Vec<&MemoryObject> = scored.into_iter().take(limit).map(|(_, m)| m).collect();

    // Estimate tokens and trim if over budget share (budget / 4 categories)
    let token_budget = budget.max_total_tokens / 4;
    let mut texts = Vec::new();
    let mut ids = Vec::new();
    let mut token_estimate = 0usize;

    for m in &selected {
        let word_count = m.text.split_whitespace().count();
        let tokens = (word_count as f64 * 1.3) as usize;
        if token_estimate + tokens > token_budget && texts.len() >= budget.min_per_category {
            break;
        }
        texts.push(m.text.clone());
        ids.push(m.memory_id.clone());
        token_estimate += tokens;
    }

    (texts, ids)
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_dialogue_turns(events: &[crate::store::EventRecord]) -> Vec<DialogueTurn> {
    let mut turns = Vec::new();
    let mut pending_prompts = VecDeque::<String>::new();

    for event in events {
        match event.event_type.as_str() {
            "stdin_captured" | "message_input" => {
                if let Some(prompt) = extract_prompt_text(event) {
                    pending_prompts.push_back(prompt);
                }
            }
            "stdout_captured" | "message_output" => {
                if let Some(response) = extract_response_text(event) {
                    if let Some(prompt) = pending_prompts.pop_front() {
                        turns.push(DialogueTurn { prompt, response });
                    }
                }
            }
            _ => {}
        }
    }

    turns
}

fn extract_prompt_text(event: &crate::store::EventRecord) -> Option<String> {
    match event.event_type.as_str() {
        "stdin_captured" => event
            .payload_json
            .get("text")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(one_line),
        _ => {
            let text = event.payload_json.get("text")?.as_str()?;
            let cleaned = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .filter(|line| !line.eq_ignore_ascii_case("/quit"))
                .filter(|line| !line.starts_with("Script "))
                .map(strip_terminal_prefix)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>();
            cleaned.last().map(|line| one_line(line))
        }
    }
}

fn extract_response_text(event: &crate::store::EventRecord) -> Option<String> {
    if event.event_type == "stdout_captured" {
        return event
            .payload_json
            .get("text")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
    }

    let text = event.payload_json.get("text")?.as_str()?;
    let mut lines: Vec<String> = Vec::new();
    let mut capturing = false;

    for raw_line in text.lines() {
        let line = one_line(raw_line.trim());
        if line.is_empty() {
            if capturing && !lines.is_empty() && !lines.last().unwrap().is_empty() {
                lines.push(String::new());
            }
            continue;
        }

        if !capturing {
            if let Some(candidate) = line
                .strip_prefix("• ")
                .or_else(|| line.strip_prefix("● "))
                .or_else(|| line.strip_prefix("✦ "))
                .map(str::trim)
            {
                if looks_like_natural_language(candidate) {
                    capturing = true;
                    lines.push(candidate.to_string());
                }
            }
            continue;
        }

        if is_terminal_ui_line(&line) {
            if !lines.is_empty() {
                break;
            }
            continue;
        }

        lines.push(line);
    }

    let response = lines
        .into_iter()
        .fold(
            Vec::<String>::new(),
            |mut acc: Vec<String>, line: String| {
                if line.is_empty() {
                    if acc.last().map(|value| value.is_empty()).unwrap_or(false) {
                        return acc;
                    }
                    acc.push(String::new());
                } else {
                    acc.push(line);
                }
                acc
            },
        )
        .join("\n")
        .trim()
        .to_string();

    if response.is_empty() {
        None
    } else {
        Some(response)
    }
}

fn extract_event_summary_text(event: &crate::store::EventRecord) -> Option<String> {
    match event.event_type.as_str() {
        "stdin_captured" | "stdout_captured" | "stderr_captured" => event
            .payload_json
            .get("text")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        "message_input" | "message_output" | "message_chunk" => event
            .payload_json
            .get("content")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("text").and_then(|value| value.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|value| !value.is_empty()),
        _ => None,
    }
}

fn strip_terminal_prefix(line: &str) -> &str {
    line.trim_start_matches(">|VTE(7600)")
        .trim_start_matches('>')
}

fn looks_like_natural_language(value: &str) -> bool {
    value.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn is_terminal_ui_line(line: &str) -> bool {
    line.starts_with('?')
        || line.starts_with("workspace ")
        || line.starts_with("sandbox")
        || line.starts_with("model")
        || line.starts_with("Shift+Tab")
        || line.starts_with("Press Ctrl+C")
        || line.starts_with("Enable ")
        || line.starts_with("Agent powering down")
        || line.starts_with("Interaction Summary")
        || line.starts_with("To resume this session:")
        || line.starts_with("Token usage:")
        || line.starts_with("Tip:")
        || line.starts_with("Starting MCP servers")
        || line.starts_with("Use /skills")
        || line.starts_with("╭")
        || line.starts_with("╰")
        || line.starts_with("│")
        || line.starts_with("─")
        || line.starts_with("▀")
        || line.starts_with("▄")
        || line.starts_with("⠋")
        || line.starts_with("⠙")
        || line.starts_with("⠹")
        || line.starts_with("⠸")
        || line.starts_with("⠼")
        || line.starts_with("⠴")
        || line.starts_with("⠦")
        || line.starts_with("⠧")
        || line.starts_with("⠇")
        || line.starts_with("⠏")
        || line.contains("GEMINI.md files")
        || line.contains("Type your message or @path/to/file")
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::{render_launcher_prompt, CheckpointManager, RehydrationPacket};
    use crate::blobs::BlobStore;
    use crate::event::{Actor, EventDraft, EventType};
    use crate::ids::{MemoryId, ProjectId, SessionId};
    use crate::journal::Journal;
    use crate::state::StatePaths;
    use crate::store::{MemoryObject, MemoryObjectKind, MetadataStore, SessionRecord};

    fn prompt_packet() -> RehydrationPacket {
        RehydrationPacket {
            schema_version: 1,
            project_identity: String::new(),
            compact_brief: String::new(),
            project_id: ProjectId::parse("prj_019d503680a475a3ae465200a90cd4fa").unwrap(),
            target_model: None,
            mission: "Continue the session faithfully from canonical state.".into(),
            stable_facts: vec![
                "Command `codex` exited with Some(0) using pty transport.".into(),
                "Frank secret santa is Dun".into(),
                "Command `gemini` exited with Some(0) using pty transport.".into(),
            ],
            constraints: Vec::new(),
            decisions: vec!["Keep this in continuity memory".into()],
            current_state:
                "Last turn used `gemini` via `gemini_cli` over `pty` and exited with Some(0)."
                    .into(),
            open_loops: vec!["Confirm the next model can recall Frank's secret santa.".into()],
            relevant_artifacts: Vec::new(),
            relevant_files: Vec::new(),
            recent_verbatim_context: vec![
                "User: what is the name of Frank's secret santa?".into(),
                "Assistant: Frank's secret santa is Dun.".into(),
            ],
            retrieval_guidance: Vec::new(),
            memory_provenance: Vec::new(),
        }
    }

    #[test]
    fn launcher_prompt_filters_operational_noise() {
        let prompt = render_launcher_prompt(&prompt_packet());
        assert!(prompt.contains("Frank secret santa is Dun"));
        assert!(prompt.contains("User: what is the name of Frank's secret santa?"));
        assert!(prompt.contains("Assistant: Frank's secret santa is Dun."));
        assert!(prompt.contains("Decisions: Keep this in continuity memory"));
        assert!(!prompt.contains("Command `codex` exited"));
        assert!(!prompt.contains("Command `gemini` exited"));
        assert!(!prompt.contains("Last turn used `gemini`"));
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
                source_adapter: None,
                source_model: None,
                importance: 0.85,
                access_count: 0,
                last_accessed_at: None,
                consolidated_from: Vec::new(),
                text_hash: None,
                valid_from: None,
                valid_to: None,
            })
            .unwrap();

        let manager = CheckpointManager::new(&state, &store, &blobs);
        let (manifest, packet) = manager
            .create_checkpoint(&journal, &session_id, "test", None)
            .unwrap();

        assert_eq!(manifest.project_id, session_id);
        assert!(packet.mission.contains("Fix the journal"));
    }

    #[test]
    fn checkpoint_prefers_recent_dialogue_from_captured_transcripts() {
        let dir = tempdir().unwrap();
        let state = StatePaths::new(dir.path().join(".contynu"));
        state.ensure_layout().unwrap();

        let store = MetadataStore::open(state.sqlite_db()).unwrap();
        let blobs = BlobStore::new(state.blobs_root());
        let session_id = SessionId::new();
        let journal = Journal::open(state.journal_path_for_session(&session_id)).unwrap();
        let turn_id = crate::ids::TurnId::new();

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
            .register_turn(&crate::store::TurnRecord {
                turn_id: turn_id.clone(),
                session_id: session_id.clone(),
                status: "completed".into(),
                started_at: chrono::Utc::now(),
                completed_at: Some(chrono::Utc::now()),
                summary_memory_id: None,
            })
            .unwrap();

        for (event_type, text) in [
            (EventType::StdinCaptured, "why is the sky blue?"),
            (
                EventType::StdoutCaptured,
                "The sky appears blue because Rayleigh scattering favors shorter wavelengths.",
            ),
            (EventType::StdinCaptured, "what did you just say about the sky?"),
            (
                EventType::StdoutCaptured,
                "I said the sky looks blue because blue light scatters more strongly in the atmosphere.",
            ),
        ] {
            let (event, append) = journal
                .append(EventDraft::new(
                    session_id.clone(),
                    Some(turn_id.clone()),
                    Actor::Runtime,
                    event_type,
                    json!({"text": text}),
                ))
                .unwrap();
            store
                .record_event(&event, &journal.path().display().to_string(), append)
                .unwrap();
        }

        let manager = CheckpointManager::new(&state, &store, &blobs);
        let packet = manager.build_packet(&session_id, None).unwrap();

        assert_eq!(packet.mission, "what did you just say about the sky?");
        assert!(packet
            .recent_verbatim_context
            .iter()
            .any(|line| line.contains("User: why is the sky blue?")));
        assert!(packet
            .recent_verbatim_context
            .iter()
            .any(|line| line.contains("Assistant: The sky appears blue because Rayleigh scattering favors shorter wavelengths.")));
        assert!(packet
            .current_state
            .contains("what did you just say about the sky?"));
    }
}
