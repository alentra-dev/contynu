use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::blobs::BlobStore;
use crate::distiller;
use crate::error::Result;
use crate::ids::{CheckpointId, ProjectId, SessionId};
use crate::rendering::one_line;
use crate::state::StatePaths;
use crate::store::{
    CheckpointRecord, MemoryObject, MemoryObjectKind, MetadataStore, WorkingSetEntry,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
            max_total_tokens: 3600,
            max_per_category: 8,
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
    #[serde(default)]
    pub user_facts: Vec<String>,
    #[serde(default)]
    pub project_knowledge: Vec<String>,
    pub relevant_artifacts: Vec<RehydrationArtifact>,
    pub relevant_files: Vec<String>,
    pub recent_verbatim_context: Vec<String>,
    pub retrieval_guidance: Vec<String>,
    #[serde(default)]
    pub recent_changes: Vec<String>,
    #[serde(default)]
    pub first_run: bool,
    #[serde(default)]
    pub memory_provenance: Vec<MemoryProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiRehydrationPacket {
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
    #[serde(default)]
    pub user_facts: Vec<String>,
    #[serde(default)]
    pub project_knowledge: Vec<String>,
    pub relevant_artifacts: Vec<RehydrationArtifact>,
    pub relevant_files: Vec<String>,
    pub recent_verbatim_context: Vec<String>,
    pub retrieval_guidance: Vec<String>,
    #[serde(default)]
    pub recent_changes: Vec<String>,
    #[serde(default)]
    pub first_run: bool,
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
        budget: &PacketBudget,
    ) -> Result<(CheckpointManifest, RehydrationPacket)> {
        let packet = self.build_packet_with_budget(session_id, target_model, budget)?;
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
        let existing_working_set = self
            .store
            .list_working_set(session_id, 12)
            .unwrap_or_default();
        let working_set_ids: HashSet<_> = existing_working_set
            .iter()
            .map(|entry| entry.memory_id.clone())
            .collect();
        let now = Utc::now();

        // Mission: latest user prompt
        let mission = recent_prompts
            .first()
            .map(|p| p.verbatim.clone())
            .unwrap_or_else(|| "Continue the session faithfully from canonical state.".into());
        let prompt_query = recent_prompts
            .first()
            .map(|p| p.verbatim.as_str())
            .unwrap_or("");

        let mut provenance = Vec::new();
        let mut accessed_ids = Vec::new();
        let slices = PacketSlices::from_budget(budget);

        let constraint_memories = select_ranked_memories(
            &memory,
            &[MemoryObjectKind::Constraint],
            prompt_query,
            slices.constraint_tokens,
            budget.max_per_category.min(6),
            now,
            &working_set_ids,
        );
        let decision_memories = select_ranked_memories(
            &memory,
            &[MemoryObjectKind::Decision],
            prompt_query,
            slices.decision_tokens,
            budget.max_per_category.min(6),
            now,
            &working_set_ids,
        );
        let open_loop_memories = select_ranked_memories(
            &memory,
            &[MemoryObjectKind::Todo],
            prompt_query,
            slices.open_loop_tokens,
            budget.max_per_category.min(6),
            now,
            &working_set_ids,
        );
        let durable_memories = select_ranked_memories(
            &memory,
            &[
                MemoryObjectKind::Fact,
                MemoryObjectKind::UserFact,
                MemoryObjectKind::ProjectKnowledge,
            ],
            prompt_query,
            slices.durable_tokens,
            budget.max_per_category.max(8),
            now,
            &working_set_ids,
        );

        let constraints =
            texts_from_memories_with_budget(&constraint_memories, slices.constraint_tokens, 90);
        let decisions =
            texts_from_memories_with_budget(&decision_memories, slices.decision_tokens, 90);
        let open_loops =
            texts_from_memories_with_budget(&open_loop_memories, slices.open_loop_tokens, 80);
        let stable_facts = texts_from_memories_with_budget(
            &durable_memories
                .iter()
                .copied()
                .filter(|m| m.kind == MemoryObjectKind::Fact)
                .collect::<Vec<_>>(),
            slices.durable_tokens / 2,
            80,
        );
        let user_facts = texts_from_memories_with_budget(
            &durable_memories
                .iter()
                .copied()
                .filter(|m| m.kind == MemoryObjectKind::UserFact)
                .collect::<Vec<_>>(),
            slices.durable_tokens / 4,
            60,
        );
        let project_knowledge = texts_from_memories_with_budget(
            &durable_memories
                .iter()
                .copied()
                .filter(|m| m.kind == MemoryObjectKind::ProjectKnowledge)
                .collect::<Vec<_>>(),
            slices.durable_tokens / 2,
            80,
        );

        for selected in [
            &constraint_memories,
            &decision_memories,
            &open_loop_memories,
            &durable_memories,
        ] {
            accessed_ids.extend(selected.iter().map(|m| m.memory_id.clone()));
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
        self.refresh_working_set(
            session_id,
            &constraint_memories,
            &decision_memories,
            &open_loop_memories,
            &durable_memories,
            now,
        );
        self.record_packet_observation(
            session_id,
            &constraint_memories,
            &decision_memories,
            &open_loop_memories,
            &durable_memories,
            budget,
            now,
        );

        // Recent verbatim context from recorded prompts
        let recent_verbatim_context =
            budget_recent_dialogue(&recent_prompts, slices.recent_dialogue_tokens);

        let first_run = memory.is_empty() && recent_prompts.is_empty();
        let recent_changes = build_recent_changes(
            &memory,
            &recent_prompts,
            prompt_query,
            now,
            slices.recent_change_tokens,
        );

        let current_state = if let Some(prompt) = recent_prompts.first() {
            format!(
                "The latest user request was: {}",
                clip_text_to_tokens(&one_line(&prompt.verbatim), slices.current_state_tokens)
            )
        } else if first_run {
            "This project is starting fresh. No prior Contynu memory has been recorded yet.".into()
        } else {
            format!("Session has {} active memory objects.", memory.len(),)
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

        // L1: Compact brief — selected memories only, ordered by relevance.
        let mut brief_lines = Vec::new();
        let mut brief_tokens = 0usize;
        let mut brief_memories = Vec::new();
        brief_memories.extend(decision_memories.iter().copied());
        brief_memories.extend(constraint_memories.iter().copied());
        brief_memories.extend(open_loop_memories.iter().copied());
        brief_memories.extend(durable_memories.iter().copied());
        dedup_memories(&mut brief_memories);

        for m in brief_memories {
            let text = compact_memory_text(&m.text, 40);
            let kind_abbrev = match m.kind {
                MemoryObjectKind::Fact => "F",
                MemoryObjectKind::Decision => "D",
                MemoryObjectKind::Constraint => "C",
                MemoryObjectKind::Todo => "T",
                MemoryObjectKind::UserFact => "U",
                MemoryObjectKind::ProjectKnowledge => "P",
            };
            let line = format!("{}: {}", kind_abbrev, text);
            let line_tokens = estimate_tokens(&line);
            if !brief_lines.is_empty() && brief_tokens + line_tokens > slices.brief_tokens {
                break;
            }
            brief_lines.push(line.clone());
            brief_tokens += line_tokens;
            if brief_lines.len() >= 8 {
                break;
            }
        }
        let compact_brief = brief_lines.join("\n");
        let mission = clip_text_to_tokens(&one_line(&mission), slices.mission_tokens);

        // Retrieval guidance — tell models how to use Contynu MCP tools
        let retrieval_guidance = vec![
            "Use the Contynu MCP tools to write memories at each stop point.".into(),
            "Call write_memory for facts, decisions, constraints worth recalling later.".into(),
            "Call record_prompt with the user's verbatim input at each stop point.".into(),
            "Call search_memory to check for existing knowledge before duplicating.".into(),
            "Call update_memory to correct or refine existing memories instead of creating duplicates.".into(),
            "Call suggest_consolidation periodically to find redundant memory clusters, then call consolidate_memories to merge them into Golden Facts.".into(),
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
            user_facts,
            project_knowledge,
            relevant_artifacts: Vec::new(),
            relevant_files: Vec::new(),
            recent_verbatim_context,
            retrieval_guidance,
            recent_changes,
            first_run,
            memory_provenance: provenance,
        })
    }

    fn refresh_working_set(
        &self,
        session_id: &SessionId,
        constraint_memories: &[&MemoryObject],
        decision_memories: &[&MemoryObject],
        open_loop_memories: &[&MemoryObject],
        durable_memories: &[&MemoryObject],
        now: DateTime<Utc>,
    ) {
        let mut ordered = Vec::new();
        ordered.extend(
            decision_memories
                .iter()
                .copied()
                .map(|m| (m, 1.0, "decision")),
        );
        ordered.extend(
            constraint_memories
                .iter()
                .copied()
                .map(|m| (m, 0.95, "constraint")),
        );
        ordered.extend(
            open_loop_memories
                .iter()
                .copied()
                .map(|m| (m, 0.9, "open_loop")),
        );
        ordered.extend(
            durable_memories
                .iter()
                .copied()
                .map(|m| (m, 0.8, "durable")),
        );

        let mut seen = HashSet::new();
        let entries: Vec<WorkingSetEntry> = ordered
            .into_iter()
            .filter(|(m, _, _)| seen.insert(m.memory_id.to_string()))
            .take(12)
            .map(|(m, rank_score, reason)| WorkingSetEntry {
                session_id: session_id.clone(),
                memory_id: m.memory_id.clone(),
                rank_score,
                source_reason: Some(reason.into()),
                refreshed_at: now,
            })
            .collect();

        let _ = self.store.replace_working_set(session_id, &entries);
    }

    fn record_packet_observation(
        &self,
        session_id: &SessionId,
        constraint_memories: &[&MemoryObject],
        decision_memories: &[&MemoryObject],
        open_loop_memories: &[&MemoryObject],
        durable_memories: &[&MemoryObject],
        budget: &PacketBudget,
        now: DateTime<Utc>,
    ) {
        let hygiene_candidates = distiller::suggest_consolidation(self.store, session_id)
            .map(|c| c.len())
            .unwrap_or(0);
        let summary = serde_json::json!({
            "session_id": session_id,
            "created_at": now.to_rfc3339(),
            "budget": {
                "max_total_tokens": budget.max_total_tokens,
                "max_per_category": budget.max_per_category,
                "min_per_category": budget.min_per_category,
            },
            "packet": {
                "constraint_count": constraint_memories.len(),
                "decision_count": decision_memories.len(),
                "open_loop_count": open_loop_memories.len(),
                "durable_count": durable_memories.len(),
                "hygiene_candidate_count": hygiene_candidates,
            },
            "selected": {
                "constraints": summarize_selected(constraint_memories),
                "decisions": summarize_selected(decision_memories),
                "open_loops": summarize_selected(open_loop_memories),
                "durable": summarize_selected(durable_memories),
            }
        });
        let _ = self
            .store
            .record_packet_observation(session_id, &summary.to_string());
    }
}

pub fn sanitize_packet(packet: &RehydrationPacket) -> AiRehydrationPacket {
    AiRehydrationPacket {
        schema_version: packet.schema_version,
        project_identity: packet.project_identity.clone(),
        compact_brief: packet.compact_brief.clone(),
        project_id: packet.project_id.clone(),
        target_model: packet.target_model.clone(),
        mission: packet.mission.clone(),
        stable_facts: packet.stable_facts.clone(),
        constraints: packet.constraints.clone(),
        decisions: packet.decisions.clone(),
        current_state: packet.current_state.clone(),
        open_loops: packet.open_loops.clone(),
        user_facts: packet.user_facts.clone(),
        project_knowledge: packet.project_knowledge.clone(),
        relevant_artifacts: packet.relevant_artifacts.clone(),
        relevant_files: packet.relevant_files.clone(),
        recent_verbatim_context: packet.recent_verbatim_context.clone(),
        retrieval_guidance: packet.retrieval_guidance.clone(),
        recent_changes: packet.recent_changes.clone(),
        first_run: packet.first_run,
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

struct PacketSlices {
    mission_tokens: usize,
    current_state_tokens: usize,
    recent_dialogue_tokens: usize,
    recent_change_tokens: usize,
    brief_tokens: usize,
    constraint_tokens: usize,
    decision_tokens: usize,
    open_loop_tokens: usize,
    durable_tokens: usize,
}

impl PacketSlices {
    fn from_budget(budget: &PacketBudget) -> Self {
        let total = budget.max_total_tokens.max(800);
        let mission_tokens = ((total * 8) / 100).clamp(80, 220);
        let current_state_tokens = ((total * 10) / 100).clamp(100, 260);
        let recent_dialogue_tokens = ((total * 9) / 100).clamp(90, 260);
        let recent_change_tokens = ((total * 10) / 100).clamp(100, 280);
        let brief_tokens = ((total * 11) / 100).clamp(140, 320);
        let reserved = mission_tokens
            + current_state_tokens
            + recent_dialogue_tokens
            + recent_change_tokens
            + brief_tokens;
        let available = total.saturating_sub(reserved).max(320);
        Self {
            mission_tokens,
            current_state_tokens,
            recent_dialogue_tokens,
            recent_change_tokens,
            brief_tokens,
            constraint_tokens: available * 18 / 100,
            decision_tokens: available * 18 / 100,
            open_loop_tokens: available * 18 / 100,
            durable_tokens: available * 46 / 100,
        }
    }
}

fn compact_memory_text(text: &str, per_item_tokens: usize) -> String {
    let one_line = one_line(text);
    clip_text_to_tokens(&one_line, per_item_tokens)
}

fn texts_from_memories_with_budget(
    memories: &[&MemoryObject],
    total_tokens: usize,
    per_item_tokens: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut used_tokens = 0usize;

    for memory in memories {
        let text = compact_memory_text(&memory.text, per_item_tokens);
        if text.is_empty() {
            continue;
        }
        let tokens = estimate_tokens(&text);
        if !out.is_empty() && used_tokens + tokens > total_tokens {
            break;
        }
        out.push(text);
        used_tokens += tokens;
    }

    out
}

fn dedup_memories(memories: &mut Vec<&MemoryObject>) {
    let mut seen = HashSet::new();
    memories.retain(|m| seen.insert(m.memory_id.to_string()));
}

fn select_ranked_memories<'a>(
    memories: &'a [MemoryObject],
    kinds: &[MemoryObjectKind],
    prompt_query: &str,
    token_budget: usize,
    max_items: usize,
    now: DateTime<Utc>,
    working_set_ids: &HashSet<crate::ids::MemoryId>,
) -> Vec<&'a MemoryObject> {
    let mut ranked: Vec<&MemoryObject> = memories
        .iter()
        .filter(|m| kinds.contains(&m.kind))
        .collect();
    ranked.sort_by(|a, b| {
        let score_a = memory_priority(a, prompt_query, now, working_set_ids);
        let score_b = memory_priority(b, prompt_query, now, working_set_ids);
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.created_at.cmp(&a.created_at))
    });

    let mut selected = Vec::new();
    let mut used_tokens = 0usize;
    for memory in ranked {
        let compact = compact_memory_text(&memory.text, 120);
        let tokens = estimate_tokens(&compact);
        if tokens > token_budget && !selected.is_empty() {
            continue;
        }
        if !selected.is_empty() && used_tokens + tokens > token_budget {
            continue;
        }
        selected.push(memory);
        used_tokens += tokens.min(token_budget);
        if selected.len() >= max_items {
            break;
        }
    }
    selected
}

fn memory_priority(
    memory: &MemoryObject,
    prompt_query: &str,
    now: DateTime<Utc>,
    working_set_ids: &HashSet<crate::ids::MemoryId>,
) -> f64 {
    let age_days = now
        .signed_duration_since(memory.updated_at.unwrap_or(memory.created_at))
        .num_hours()
        .max(0) as f64
        / 24.0;
    let recency = 1.0 / (1.0 + age_days / 7.0);
    let lexical = lexical_relevance(prompt_query, &memory.text);
    let access = ((memory.access_count.min(10) as f64) / 10.0).clamp(0.0, 1.0);
    let scope = match memory.scope {
        crate::store::MemoryScope::Project => 1.0,
        crate::store::MemoryScope::User => 0.9,
        crate::store::MemoryScope::Session => 0.75,
    };
    let working_set_boost = if working_set_ids.contains(&memory.memory_id) {
        0.12
    } else {
        0.0
    };

    (memory.importance * 0.4)
        + (lexical * 0.3)
        + (recency * 0.1)
        + (access * 0.05)
        + (scope * 0.03)
        + working_set_boost
}

fn lexical_relevance(query: &str, text: &str) -> f64 {
    let query_terms = normalize_terms(query);
    if query_terms.is_empty() {
        return 0.0;
    }
    let text_terms = normalize_terms(text);
    if text_terms.is_empty() {
        return 0.0;
    }
    let overlap = query_terms.intersection(&text_terms).count() as f64;
    overlap / query_terms.len() as f64
}

fn normalize_terms(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 3)
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn estimate_tokens(text: &str) -> usize {
    ((text.split_whitespace().count() as f64) * 1.3).ceil() as usize
}

fn clip_text_to_tokens(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return String::new();
    }

    let words = text.split_whitespace().collect::<Vec<_>>();
    let max_words = ((max_tokens as f64) / 1.3).floor().max(1.0) as usize;
    if words.len() <= max_words {
        return text.trim().to_string();
    }

    let clipped = words[..max_words].join(" ");
    format!("{}...", clipped.trim())
}

fn summarize_selected(memories: &[&MemoryObject]) -> Vec<serde_json::Value> {
    memories
        .iter()
        .map(|memory| {
            serde_json::json!({
                "memory_id": memory.memory_id.as_str(),
                "kind": memory.kind.as_str(),
                "importance": memory.importance,
                "source": memory.source_model,
                "text_preview": one_line_preview(&memory.text, 120),
            })
        })
        .collect()
}

fn one_line_preview(text: &str, max_bytes: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.len() <= max_bytes {
        one_line
    } else {
        let truncated =
            crate::text::truncate_at_char_boundary(&one_line, max_bytes.saturating_sub(3));
        format!("{}...", truncated)
    }
}

fn build_recent_changes(
    memories: &[MemoryObject],
    recent_prompts: &[crate::store::PromptRecord],
    prompt_query: &str,
    now: DateTime<Utc>,
    total_tokens: usize,
) -> Vec<String> {
    let mut changes = Vec::new();
    let mut used_tokens = 0usize;
    let empty_working_set = HashSet::new();

    if let Some(prompt) = recent_prompts.first() {
        let latest = format!(
            "Latest user request: {}",
            clip_text_to_tokens(&one_line(&prompt.verbatim), 80)
        );
        used_tokens += estimate_tokens(&latest);
        changes.push(latest);
    }

    let mut recent_memories: Vec<&MemoryObject> = memories.iter().collect();
    recent_memories.sort_by(|a, b| {
        let score_a = memory_priority(a, prompt_query, now, &empty_working_set);
        let score_b = memory_priority(b, prompt_query, now, &empty_working_set);
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for memory in recent_memories {
        let label = match memory.kind {
            MemoryObjectKind::Decision => Some("Decision"),
            MemoryObjectKind::Constraint => Some("Constraint"),
            MemoryObjectKind::Todo => Some("Open thread"),
            MemoryObjectKind::Fact => Some("Fact"),
            MemoryObjectKind::UserFact => Some("User fact"),
            MemoryObjectKind::ProjectKnowledge => Some("Project knowledge"),
        };

        if let Some(label) = label {
            let text = memory.text.trim();
            if text.is_empty() {
                continue;
            }
            let line = format!("{label}: {}", compact_memory_text(text, 64));
            let line_tokens = estimate_tokens(&line);
            if !changes.is_empty() && used_tokens + line_tokens > total_tokens {
                break;
            }
            if !changes.contains(&line) {
                changes.push(line);
                used_tokens += line_tokens;
            }
        }

        if changes.len() >= 6 {
            break;
        }
    }

    changes
}

fn budget_recent_dialogue(
    recent_prompts: &[crate::store::PromptRecord],
    total_tokens: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut used_tokens = 0usize;

    for prompt in recent_prompts.iter().rev().take(3) {
        let mut line = format!(
            "User: {}",
            clip_text_to_tokens(&one_line(&prompt.verbatim), 70)
        );
        if let Some(ref interp) = prompt.interpretation {
            let interp = clip_text_to_tokens(&one_line(interp), 30);
            line.push_str(&format!(" (interpreted as: {interp})"));
        }
        let tokens = estimate_tokens(&line);
        if !out.is_empty() && used_tokens + tokens > total_tokens {
            break;
        }
        out.push(line);
        used_tokens += tokens;
    }

    out
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{
        clip_text_to_tokens, one_line_preview, render_launcher_prompt, sanitize_packet,
        CheckpointManager, PacketBudget, RehydrationPacket,
    };
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
            stable_facts: vec!["Frank secret santa is Dun".into()],
            constraints: Vec::new(),
            decisions: vec!["Keep this in continuity memory".into()],
            current_state: "The latest user request was: what is the name of Frank's secret santa?"
                .into(),
            open_loops: vec!["Confirm the next model can recall Frank's secret santa.".into()],
            user_facts: Vec::new(),
            project_knowledge: Vec::new(),
            relevant_artifacts: Vec::new(),
            relevant_files: Vec::new(),
            recent_verbatim_context: vec!["User: what is the name of Frank's secret santa?".into()],
            retrieval_guidance: Vec::new(),
            recent_changes: vec![
                "Latest user request: what is the name of Frank's secret santa?".into(),
            ],
            first_run: false,
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
            "Last Focus: The user corrected the spelling of Frank's name and wants that remembered."
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
            .create_checkpoint(&session_id, "test", None, &PacketBudget::default())
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
            (
                "what did you just say about the sky?",
                Some("Recalling previous answer about Rayleigh scattering"),
            ),
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
        assert!(
            packet
                .recent_changes
                .iter()
                .any(|line| line
                    .contains("Latest user request: what did you just say about the sky?"))
        );
    }

    #[test]
    fn packet_budget_truncates_large_prompt_and_memory_payloads() {
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
                cli_name: Some("gemini".into()),
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
                prompt_id: "pmt_big".into(),
                session_id: session_id.clone(),
                verbatim: "this is a very long prompt ".repeat(500),
                interpretation: Some("continue prior work".into()),
                interpretation_confidence: Some(0.9),
                source_model: None,
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        let huge_memory = "operational context and prior transcript details ".repeat(700);
        for kind in [
            MemoryObjectKind::Constraint,
            MemoryObjectKind::Decision,
            MemoryObjectKind::ProjectKnowledge,
        ] {
            store
                .insert_memory_object(&MemoryObject {
                    memory_id: MemoryId::new(),
                    session_id: session_id.clone(),
                    kind,
                    scope: MemoryScope::Project,
                    status: "active".into(),
                    text: huge_memory.clone(),
                    importance: 0.9,
                    reason: None,
                    source_model: None,
                    superseded_by: None,
                    created_at: chrono::Utc::now(),
                    updated_at: None,
                    access_count: 0,
                    last_accessed_at: None,
                })
                .unwrap();
        }

        let manager = CheckpointManager::new(&state, &store, &blobs);
        let packet = manager
            .build_packet_with_budget(&session_id, None, &PacketBudget::default())
            .unwrap();

        let packet_json = serde_json::to_string(&packet).unwrap();
        assert!(
            packet_json.len() < 40_000,
            "packet too large: {}",
            packet_json.len()
        );
        assert!(packet.mission.ends_with("..."));
        assert!(packet
            .constraints
            .first()
            .map(|line| line.ends_with("..."))
            .unwrap_or(false));
        assert!(packet.recent_verbatim_context.len() <= 3);
    }

    #[test]
    fn build_packet_marks_first_run_when_no_memory_exists() {
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
                cli_name: Some("codex_cli".into()),
                cli_version: None,
                model_name: None,
                cwd: None,
                repo_root: None,
                host_fingerprint: None,
                started_at: chrono::Utc::now(),
                ended_at: None,
            })
            .unwrap();

        let manager = CheckpointManager::new(&state, &store, &blobs);
        let packet = manager.build_packet(&session_id, None).unwrap();

        assert!(packet.first_run);
        assert!(packet.current_state.contains("starting fresh"));
        assert!(packet.recent_changes.is_empty());
    }

    #[test]
    fn build_packet_prefers_prompt_relevant_memory_under_budget() {
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
                cli_name: Some("codex_cli".into()),
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
                prompt_id: "pmt_rel".into(),
                session_id: session_id.clone(),
                verbatim: "Fix the authentication bug in the login flow".into(),
                interpretation: None,
                interpretation_confidence: None,
                source_model: None,
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        for (text, importance) in [
            (
                "Authentication bug occurs in the login flow because the token parser trims the bearer prefix incorrectly",
                0.65,
            ),
            (
                "The project mascot is a blue heron used in marketing screenshots and stickers",
                0.95,
            ),
        ] {
            store
                .insert_memory_object(&MemoryObject {
                    memory_id: MemoryId::new(),
                    session_id: session_id.clone(),
                    kind: MemoryObjectKind::Fact,
                    scope: MemoryScope::Project,
                    status: "active".into(),
                    text: text.into(),
                    importance,
                    reason: None,
                    source_model: None,
                    superseded_by: None,
                    created_at: chrono::Utc::now(),
                    updated_at: None,
                    access_count: 0,
                    last_accessed_at: None,
                })
                .unwrap();
        }

        let manager = CheckpointManager::new(&state, &store, &blobs);
        let packet = manager
            .build_packet_with_budget(
                &session_id,
                None,
                &PacketBudget {
                    max_total_tokens: 260,
                    max_per_category: 4,
                    min_per_category: 1,
                },
            )
            .unwrap();

        assert!(packet
            .stable_facts
            .iter()
            .any(|fact| fact.contains("Authentication bug occurs in the login flow")));
    }

    #[test]
    fn build_packet_promotes_ingested_external_memory_into_working_set() {
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
                cli_name: Some("codex_cli".into()),
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
                prompt_id: "pmt_ingested".into(),
                session_id: session_id.clone(),
                verbatim: "Continue debugging the authentication bug".into(),
                interpretation: None,
                interpretation_confidence: None,
                source_model: None,
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        let external_memory_id = MemoryId::new();
        store
            .insert_memory_object(&MemoryObject {
                memory_id: external_memory_id.clone(),
                session_id: session_id.clone(),
                kind: MemoryObjectKind::ProjectKnowledge,
                scope: MemoryScope::Project,
                status: "active".into(),
                text:
                    "Authentication bug narrowed down to the bearer token parser in the login flow"
                        .into(),
                importance: 0.72,
                reason: Some("Ingested from Codex history session abc123".into()),
                source_model: Some("codex".into()),
                superseded_by: None,
                created_at: chrono::Utc::now(),
                updated_at: None,
                access_count: 0,
                last_accessed_at: None,
            })
            .unwrap();

        let manager = CheckpointManager::new(&state, &store, &blobs);
        let packet = manager.build_packet(&session_id, None).unwrap();
        assert!(packet
            .project_knowledge
            .iter()
            .any(|item| item.contains("bearer token parser")));

        let working_set = store.list_working_set(&session_id, 10).unwrap();
        assert!(working_set
            .iter()
            .any(|entry| entry.memory_id == external_memory_id));
    }

    #[test]
    fn sanitize_packet_removes_provenance_for_ai_delivery() {
        let mut packet = prompt_packet();
        packet
            .memory_provenance
            .push(crate::checkpoint::MemoryProvenance {
                memory_id: "mem_123".into(),
                kind: "fact".into(),
                source_model: Some("codex/gpt-5".into()),
                importance: 0.9,
            });

        let value = serde_json::to_value(sanitize_packet(&packet)).unwrap();
        assert!(value.get("memory_provenance").is_none());
        let rendered = serde_json::to_string(&value).unwrap();
        assert!(!rendered.contains("source_model"));
    }

    #[test]
    fn one_line_preview_handles_multibyte_boundary_safety() {
        // '─' (U+2500) is 3 bytes in UTF-8.
        // 116..119 is where it occupies.
        // Truncating to 117 would panic if not handled.
        let mut s = "a".repeat(116);
        s.push('─');
        s.push_str(" rest of text");

        let preview = one_line_preview(&s, 120);
        // Should not panic, should truncate to "a"*116 + "..."
        assert!(preview.ends_with("..."));
        assert_eq!(preview.len(), 116 + 3);
    }

    #[test]
    fn clip_text_to_tokens_preserves_short_text_and_truncates_long_text() {
        assert_eq!(clip_text_to_tokens("short text", 10), "short text");
        let clipped = clip_text_to_tokens("one two three four five six seven eight", 3);
        assert!(clipped.ends_with("..."));
    }
}
