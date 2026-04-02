use std::fs;
use std::path::Path;
use std::{collections::VecDeque, fmt::Write as _};

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
        let events = self.store.list_events_for_session(session_id)?;
        let memory = self.store.list_memory_objects(session_id, None)?;
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

        let stable_facts = trim_list(memory_texts(&memory, MemoryObjectKind::Fact), 8);
        let constraints = trim_list(memory_texts(&memory, MemoryObjectKind::Constraint), 8);
        let decisions = trim_list(memory_texts(&memory, MemoryObjectKind::Decision), 8);
        let open_loops = trim_list(memory_texts(&memory, MemoryObjectKind::Todo), 8);
        let relevant_files = Vec::new();
        let recent_verbatim_context = if !dialogue_turns.is_empty() {
            dialogue_turns
                .iter()
                .rev()
                .take(3)
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
                .take(8)
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
                .rev()
                .take(8)
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
        })
    }
}

pub fn render_rehydration_prompt(packet: &RehydrationPacket, adapter_name: &str) -> String {
    let mut prompt = String::new();
    let _ = writeln!(prompt, "Contynu continuity context for {}.", adapter_name);
    prompt
        .push_str("Use this as authoritative project memory carried forward from prior work.\n\n");
    prompt.push_str("Project\n\n");
    let _ = writeln!(prompt, "- ID: {}", packet.project_id);
    if let Some(target_model) = packet.target_model.as_deref() {
        let _ = writeln!(prompt, "- Target model: {}", target_model);
    }
    let _ = writeln!(prompt, "- Schema version: {}", packet.schema_version);
    prompt.push('\n');

    prompt.push_str("Mission\n\n");
    let _ = writeln!(prompt, "- {}", packet.mission);
    prompt.push('\n');

    prompt.push_str("Current State\n\n");
    let _ = writeln!(prompt, "- {}", packet.current_state);
    prompt.push('\n');

    write_bullets(
        &mut prompt,
        "Recent Conversation",
        &packet.recent_verbatim_context,
    );
    write_bullets(&mut prompt, "Stable Facts", &packet.stable_facts);
    write_bullets(&mut prompt, "Constraints", &packet.constraints);
    write_bullets(&mut prompt, "Decisions", &packet.decisions);
    write_bullets(&mut prompt, "Open Loops", &packet.open_loops);
    write_bullets(&mut prompt, "Relevant Files", &packet.relevant_files);

    let artifact_lines = packet
        .relevant_artifacts
        .iter()
        .map(|artifact| {
            format!(
                "{} | {} | {}",
                artifact.kind, artifact.path, artifact.sha256
            )
        })
        .collect::<Vec<_>>();
    write_bullets(&mut prompt, "Relevant Artifacts", &artifact_lines);
    write_bullets(
        &mut prompt,
        "Retrieval Guidance",
        &packet.retrieval_guidance,
    );

    prompt.push_str(
        "Carry this continuity forward naturally. If the user asks about prior work, answer from this memory instead of claiming there is no earlier context.\n",
    );
    prompt
}

pub fn render_launcher_prompt(packet: &RehydrationPacket) -> String {
    let mut sections = Vec::new();

    sections.push(format!(
        "Continue this Contynu project with prior memory. Project: {}.",
        packet.project_id
    ));
    sections.push(format!("Mission: {}", one_line(&packet.mission)));
    sections.push(format!(
        "Current state: {}",
        one_line(&packet.current_state)
    ));

    if !packet.recent_verbatim_context.is_empty() {
        let recent = packet
            .recent_verbatim_context
            .iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|item| one_line(item))
            .collect::<Vec<_>>()
            .join(" | ");
        sections.push(format!("Recent conversation: {}", recent));
    }

    if !packet.stable_facts.is_empty() {
        let facts = packet
            .stable_facts
            .iter()
            .take(3)
            .map(|item| one_line(item))
            .collect::<Vec<_>>()
            .join(" | ");
        sections.push(format!("Stable facts: {}", facts));
    }

    if !packet.open_loops.is_empty() {
        let open_loops = packet
            .open_loops
            .iter()
            .take(3)
            .map(|item| one_line(item))
            .collect::<Vec<_>>()
            .join(" | ");
        sections.push(format!("Open loops: {}", open_loops));
    }

    sections.push(
        "Use this as prior context, but do not restate it unless relevant. If exact history is needed, use the Contynu rehydration files from the environment."
            .into(),
    );

    sections.join("\n")
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

fn write_bullets(buffer: &mut String, title: &str, items: &[String]) {
    buffer.push_str(title);
    buffer.push_str("\n\n");
    if items.is_empty() {
        buffer.push_str("- None recorded.\n\n");
        return;
    }
    for item in items {
        let _ = writeln!(buffer, "- {}", one_line(item));
    }
    buffer.push('\n');
}

fn trim_list(items: Vec<String>, limit: usize) -> Vec<String> {
    if items.len() <= limit {
        items
    } else {
        items
            .into_iter()
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
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
