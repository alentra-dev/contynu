use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::checkpoint::RehydrationPacket;
use crate::store::{MemoryObject, MemoryObjectKind};

/// Format used to render rehydration prompts for different LLM providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptFormat {
    /// XML-structured context (optimal for Claude).
    Xml,
    /// Markdown with headers and bullet lists (optimal for GPT/Codex).
    Markdown,
    /// Plain labeled sections (Gemini and generic fallback).
    StructuredText,
}

/// Hard ceiling on a rendered rehydration prompt, in bytes. Well under the
/// Linux `MAX_ARG_STRLEN` (128 KiB) so a prompt can never blow up `execve`,
/// and comfortable inside every current LLM context window. The packet budget
/// keeps us far below this in normal operation; this is a last-line guard so
/// a single oversized memory never silently breaks continuity.
pub const MAX_RENDERED_PROMPT_BYTES: usize = 32 * 1024;

/// Render a full rehydration prompt in the given format.
pub fn render_rehydration(
    packet: &RehydrationPacket,
    format: PromptFormat,
    adapter_name: &str,
) -> String {
    let rendered = match format {
        PromptFormat::Xml => render_xml(packet, adapter_name),
        PromptFormat::Markdown => {
            if adapter_name == "codex_cli" {
                render_codex_markdown(packet)
            } else {
                render_markdown(packet, adapter_name)
            }
        }
        PromptFormat::StructuredText => render_structured_text(packet, adapter_name),
    };
    cap_rendered_prompt(rendered, MAX_RENDERED_PROMPT_BYTES)
}

fn cap_rendered_prompt(rendered: String, max_bytes: usize) -> String {
    if rendered.len() <= max_bytes {
        return rendered;
    }
    let trailer =
        "\n\n[contynu: context truncated — call the `search_memory` MCP tool for more.]\n";
    let budget = max_bytes.saturating_sub(trailer.len());
    let clipped = crate::text::truncate_at_char_boundary(&rendered, budget);
    format!("{clipped}{trailer}")
}

/// Render a compact launcher prompt in the given format.
pub fn render_launcher(packet: &RehydrationPacket, format: PromptFormat) -> String {
    // Launcher prompts are always compact one-liners regardless of format,
    // but we include format-appropriate framing.
    match format {
        PromptFormat::Xml => render_launcher_xml(packet),
        PromptFormat::Markdown => render_launcher_markdown(packet),
        PromptFormat::StructuredText => render_launcher_structured(packet),
    }
}

// ---------------------------------------------------------------------------
// XML rendering (Claude)
// ---------------------------------------------------------------------------

fn render_xml(packet: &RehydrationPacket, adapter_name: &str) -> String {
    let mut out = String::new();
    let recent_dialogue = dedup_lines(packet.recent_verbatim_context.iter().map(|s| s.as_str()), 3);
    let retrieval_guidance = dedup_lines(packet.retrieval_guidance.iter().map(|s| s.as_str()), 5);
    let _ = writeln!(
        out,
        "<contynu_memory project=\"{}\" schema=\"{}\" adapter=\"{}\">",
        packet.project_id, packet.schema_version, adapter_name
    );

    if !packet.project_identity.is_empty() {
        let _ = writeln!(
            out,
            "  <identity>{}</identity>",
            xml_escape(&packet.project_identity)
        );
    }
    if !packet.compact_brief.is_empty() {
        out.push_str("  <brief>\n");
        for line in packet.compact_brief.lines() {
            let _ = writeln!(out, "    {}", xml_escape(line));
        }
        out.push_str("  </brief>\n");
    }

    if let Some(target) = &packet.target_model {
        let _ = writeln!(out, "  <target_model>{target}</target_model>");
    }

    let _ = writeln!(out, "  <mission>{}</mission>", xml_escape(&packet.mission));
    let _ = writeln!(
        out,
        "  <current_state>{}</current_state>",
        xml_escape(&packet.current_state)
    );

    write_xml_section(&mut out, "facts", "fact", &packet.stable_facts);
    write_xml_section(&mut out, "constraints", "constraint", &packet.constraints);
    write_xml_section(&mut out, "decisions", "decision", &packet.decisions);
    write_xml_section(&mut out, "open_loops", "todo", &packet.open_loops);
    write_xml_section(&mut out, "user_facts", "user_fact", &packet.user_facts);
    write_xml_section(
        &mut out,
        "project_knowledge",
        "knowledge",
        &packet.project_knowledge,
    );

    if !recent_dialogue.is_empty() {
        out.push_str("  <recent_dialogue>\n");
        for line in &recent_dialogue {
            let (role, text) = if let Some(rest) = line.strip_prefix("User: ") {
                ("user", rest)
            } else if let Some(rest) = line.strip_prefix("Assistant: ") {
                ("assistant", rest)
            } else {
                ("system", line.as_str())
            };
            let _ = writeln!(out, "    <turn role=\"{role}\">{}</turn>", xml_escape(text));
        }
        out.push_str("  </recent_dialogue>\n");
    }

    if !packet.relevant_artifacts.is_empty() {
        out.push_str("  <artifacts>\n");
        for artifact in &packet.relevant_artifacts {
            let _ = writeln!(
                out,
                "    <artifact kind=\"{}\" path=\"{}\" sha256=\"{}\" />",
                xml_escape(&artifact.kind),
                xml_escape(&artifact.path),
                artifact.sha256
            );
        }
        out.push_str("  </artifacts>\n");
    }

    if !packet.relevant_files.is_empty() {
        out.push_str("  <files>\n");
        for file in &packet.relevant_files {
            let _ = writeln!(out, "    <file>{}</file>", xml_escape(file));
        }
        out.push_str("  </files>\n");
    }

    if !retrieval_guidance.is_empty() {
        out.push_str("  <guidance>\n");
        for item in &retrieval_guidance {
            let _ = writeln!(out, "    <item>{}</item>", xml_escape(item));
        }
        out.push_str("  </guidance>\n");
    }

    out.push_str("  <instruction>\n");
    out.push_str("    You have Contynu MCP tools available. Use them to manage your memory:\n");
    out.push_str("    - record_prompt: ALWAYS call this with the user's verbatim prompt at each generation stop. If the prompt is ambiguous, include your interpretation.\n");
    out.push_str("    - write_memory: Write facts, decisions, constraints, or knowledge worth recalling in future sessions. You decide what is worth remembering from your own output. Kinds: fact, constraint, decision, todo, user_fact, project_knowledge. Scopes: user (follows the user everywhere), project (this project only), session (ephemeral).\n");
    out.push_str("    - update_memory: Correct or refine an existing memory by its ID instead of creating duplicates.\n");
    out.push_str("    - delete_memory: Remove a memory that is no longer relevant.\n");
    out.push_str(
        "    - search_memory: Search existing memories before writing to avoid duplicates.\n",
    );
    out.push_str("    - list_memories: Browse all active memories.\n");
    out.push_str("    - suggest_consolidation: Scan for redundant memory clusters that can be merged into Golden Facts.\n");
    out.push_str("    - consolidate_memories: Merge related memories into a single high-fidelity Golden Fact. Originals are superseded, not deleted.\n");
    out.push_str("    Carry this continuity forward naturally. If the user asks about prior work, answer from this memory instead of claiming there is no earlier context.\n");
    out.push_str("  </instruction>\n");
    out.push_str("</contynu_memory>\n");
    out
}

fn write_xml_section(out: &mut String, section: &str, item_tag: &str, items: &[String]) {
    let _ = writeln!(out, "  <{section}>");
    if items.is_empty() {
        let _ = writeln!(out, "    <{item_tag}>None recorded.</{item_tag}>");
    } else {
        for item in items {
            let _ = writeln!(out, "    <{item_tag}>{}</{item_tag}>", xml_escape(item));
        }
    }
    let _ = writeln!(out, "  </{section}>");
}

fn render_launcher_xml(packet: &RehydrationPacket) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "<contynu_context project=\"{}\" schema=\"{}\">",
        packet.project_id, packet.schema_version
    );
    if !packet.mission.is_empty()
        && packet.mission != "Continue the session faithfully from canonical state."
    {
        let _ = write!(
            out,
            "<mission>{}</mission>",
            xml_escape(&one_line(&packet.mission))
        );
    }
    if !packet.current_state.is_empty() {
        let _ = write!(
            out,
            "<state>{}</state>",
            xml_escape(&one_line(&packet.current_state))
        );
    }
    out.push_str("</contynu_context>");
    out
}

// ---------------------------------------------------------------------------
// Markdown rendering (GPT/Codex)
// ---------------------------------------------------------------------------

fn render_markdown(packet: &RehydrationPacket, adapter_name: &str) -> String {
    let mut out = String::new();
    let recent_dialogue = dedup_lines(packet.recent_verbatim_context.iter().map(|s| s.as_str()), 3);
    let durable_context = combined_context(packet);
    let _ = writeln!(out, "# Contynu Memory Context");
    let _ = writeln!(
        out,
        "**Project:** {} | **Schema:** {} | **Adapter:** {}\n",
        packet.project_id, packet.schema_version, adapter_name
    );

    if !packet.project_identity.is_empty() {
        let _ = writeln!(out, "> {}\n", packet.project_identity);
    }
    if !packet.compact_brief.is_empty() {
        out.push_str("## Quick Brief\n```\n");
        out.push_str(&packet.compact_brief);
        out.push_str("\n```\n\n");
    }

    if let Some(target) = &packet.target_model {
        let _ = writeln!(out, "**Target model:** {target}\n");
    }

    let _ = writeln!(out, "## Mission\n{}\n", packet.mission);
    let _ = writeln!(out, "## Current State\n{}\n", packet.current_state);

    write_md_section(&mut out, "Constraints", &packet.constraints);
    write_md_section(&mut out, "Decisions", &packet.decisions);
    write_md_section(&mut out, "Open Loops", &packet.open_loops);
    write_md_section(&mut out, "Durable Context", &durable_context);

    if !recent_dialogue.is_empty() {
        out.push_str("## Recent Dialogue\n\n");
        for line in &recent_dialogue {
            if let Some(rest) = line.strip_prefix("User: ") {
                let _ = writeln!(out, "> **User:** {rest}");
            } else if let Some(rest) = line.strip_prefix("Assistant: ") {
                let _ = writeln!(out, "> **Assistant:** {rest}");
            } else {
                let _ = writeln!(out, "> {line}");
            }
        }
        out.push('\n');
    }

    if !packet.relevant_artifacts.is_empty() {
        out.push_str("## Relevant Artifacts\n\n");
        out.push_str("| Kind | Path | SHA256 |\n|------|------|--------|\n");
        for artifact in &packet.relevant_artifacts {
            let _ = writeln!(
                out,
                "| {} | {} | `{}` |",
                artifact.kind,
                artifact.path,
                &artifact.sha256[..16.min(artifact.sha256.len())]
            );
        }
        out.push('\n');
    }

    write_md_section(&mut out, "Relevant Files", &packet.relevant_files);
    write_md_section(&mut out, "Retrieval Guidance", &packet.retrieval_guidance);

    out.push_str("---\n\n## How to Use Contynu Memory\n\n");
    out.push_str("You have Contynu MCP tools available. Use them to manage your memory:\n\n");
    out.push_str("- **record_prompt**: ALWAYS call this with the user's verbatim prompt at each generation stop. If the prompt is ambiguous, include your interpretation.\n");
    out.push_str("- **write_memory**: Write facts, decisions, constraints, or knowledge worth recalling in future sessions. You decide what is worth remembering from your own output. Kinds: `fact`, `constraint`, `decision`, `todo`, `user_fact`, `project_knowledge`. Scopes: `user` (follows the user everywhere), `project` (this project only), `session` (ephemeral).\n");
    out.push_str("- **update_memory**: Correct or refine an existing memory by its ID instead of creating duplicates.\n");
    out.push_str("- **delete_memory**: Remove a memory that is no longer relevant.\n");
    out.push_str(
        "- **search_memory**: Search existing memories before writing to avoid duplicates.\n",
    );
    out.push_str("- **list_memories**: Browse all active memories.\n");
    out.push_str("- **suggest_consolidation**: Scan for redundant memory clusters that can be merged into Golden Facts.\n");
    out.push_str("- **consolidate_memories**: Merge related memories into a single high-fidelity Golden Fact. Originals are superseded, not deleted.\n\n");
    out.push_str("*Carry this continuity forward naturally. If the user asks about prior work, answer from this memory instead of claiming there is no earlier context.*\n");
    out
}

fn render_codex_markdown(packet: &RehydrationPacket) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Contynu Working Continuation");
    let _ = writeln!(
        out,
        "**Project:** {} | **Schema:** {}\n",
        packet.project_id, packet.schema_version
    );

    out.push_str("> Read this as carried-forward working state for this run.\n");
    out.push_str("> Use it directly. Do not summarize it back unless it becomes relevant.\n\n");

    if packet.first_run {
        out.push_str("## Startup Mode\n");
        out.push_str("- This project has no prior Contynu memory yet.\n");
        out.push_str(
            "- Treat the repository itself and the user's next requests as the source of truth.\n",
        );
        out.push_str("- As durable facts, constraints, decisions, and todos emerge, record them with Contynu tools.\n\n");
    }

    let _ = writeln!(out, "## Current Goal\n{}\n", packet.mission.trim());
    let _ = writeln!(out, "## Active State\n{}\n", packet.current_state.trim());

    let mut latest_changes = Vec::new();
    if let Some(target) = &packet.target_model {
        latest_changes.push(format!("Target model for this handoff: {target}"));
    }
    latest_changes.extend(packet.recent_changes.iter().map(|line| one_line(line)));
    if latest_changes.is_empty() {
        latest_changes.extend(
            packet
                .recent_verbatim_context
                .iter()
                .filter(|line| !is_operational(line))
                .map(|line| one_line(line))
                .take(4),
        );
    }
    write_md_section(&mut out, "What Changed", &latest_changes);
    write_md_section(&mut out, "Constraints In Force", &packet.constraints);
    write_md_section(&mut out, "Decisions In Force", &packet.decisions);
    write_md_section(&mut out, "Open Threads", &packet.open_loops);

    let mut durable_context = Vec::new();
    durable_context.extend(packet.stable_facts.iter().cloned());
    durable_context.extend(packet.project_knowledge.iter().cloned());
    durable_context.extend(packet.user_facts.iter().cloned());
    write_md_section(&mut out, "Durable Context", &durable_context);

    if !packet.relevant_files.is_empty() {
        write_md_section(&mut out, "Relevant Files", &packet.relevant_files);
    }

    if !packet.relevant_artifacts.is_empty() {
        out.push_str("## Relevant Artifacts\n\n");
        for artifact in &packet.relevant_artifacts {
            let _ = writeln!(
                out,
                "- {}: {} (`{}`)",
                artifact.kind,
                artifact.path,
                &artifact.sha256[..16.min(artifact.sha256.len())]
            );
        }
        out.push('\n');
    }

    out.push_str("## Contynu Usage\n\n");
    out.push_str("- Search or browse memory before duplicating context.\n");
    out.push_str("- Record the user's prompt at each stop point.\n");
    out.push_str(
        "- Write only durable facts, decisions, constraints, todos, or project knowledge.\n",
    );
    out.push_str("- Update existing memories instead of rewriting the same idea.\n");
    out.push_str(
        "- Use consolidation tools only when multiple active memories are clearly redundant.\n\n",
    );
    out.push_str("*Continue the work from this state. Treat repository instructions outside this block as still in force.*\n");
    out
}

fn write_md_section(out: &mut String, title: &str, items: &[String]) {
    let _ = writeln!(out, "## {title}\n");
    if items.is_empty() {
        out.push_str("- None recorded.\n\n");
        return;
    }
    for item in items {
        let _ = writeln!(out, "- {}", one_line(item));
    }
    out.push('\n');
}

fn render_launcher_markdown(packet: &RehydrationPacket) -> String {
    let mut sections = Vec::new();
    sections.push(format!(
        "**Contynu** project `{}` (schema {}).",
        packet.project_id, packet.schema_version
    ));
    if !packet.mission.is_empty()
        && packet.mission != "Continue the session faithfully from canonical state."
    {
        sections.push(format!("**Mission:** {}", one_line(&packet.mission)));
    }
    if !packet.current_state.is_empty() {
        sections.push(format!("**State:** {}", one_line(&packet.current_state)));
    }
    sections.push("Use this as prior context, but do not restate it unless relevant.".into());
    sections.join(" ")
}

// ---------------------------------------------------------------------------
// Structured text rendering (Gemini / generic fallback)
// ---------------------------------------------------------------------------

fn render_structured_text(packet: &RehydrationPacket, adapter_name: &str) -> String {
    let mut prompt = String::new();
    let durable_context = combined_context(packet);
    let recent_dialogue = dedup_lines(packet.recent_verbatim_context.iter().map(|s| s.as_str()), 3);

    // Quick-reference summary at the top so the model sees key facts immediately.
    prompt.push_str("IMPORTANT: This file contains project memory from prior sessions. READ THIS FIRST before searching files.\n\n");
    if !packet.project_identity.is_empty() {
        let _ = writeln!(prompt, "{}\n", packet.project_identity);
    }
    if !packet.compact_brief.is_empty() {
        prompt.push_str("## QUICK BRIEF\n\n");
        for line in packet.compact_brief.lines() {
            let _ = writeln!(prompt, "  {}", line);
        }
        prompt.push('\n');
    }
    if !durable_context.is_empty() {
        prompt.push_str("## KEY CONTEXT FROM PRIOR SESSIONS\n\n");
        for fact in &durable_context {
            if !is_operational(fact) {
                let _ = writeln!(prompt, "  * {}", one_line(fact));
            }
        }
        prompt.push('\n');
    }

    prompt.push_str("## ROLE\n\n");
    let _ = writeln!(
        prompt,
        "You are {} powered by Contynu, an AI agent with persistent cross-session memory.",
        adapter_name
    );
    prompt.push_str("Your primary goal is to carry forward the current project mission by synthesizing prior knowledge and maintaining state across generations.\n\n");

    prompt.push_str("## CONTEXT\n\n");
    prompt
        .push_str("Use this as authoritative project memory carried forward from prior work.\n\n");

    prompt.push_str("### Project\n\n");
    let _ = writeln!(prompt, "- ID: {}", packet.project_id);
    if let Some(target_model) = packet.target_model.as_deref() {
        let _ = writeln!(prompt, "- Target model: {}", target_model);
    }
    let _ = writeln!(prompt, "- Schema version: {}", packet.schema_version);
    prompt.push('\n');

    prompt.push_str("### Mission\n\n");
    let _ = writeln!(prompt, "{}", packet.mission);
    prompt.push('\n');

    prompt.push_str("### Current State\n\n");
    let _ = writeln!(prompt, "{}", packet.current_state);
    prompt.push('\n');

    write_st_bullets(&mut prompt, "### Recent Conversation", &recent_dialogue);
    write_st_bullets(&mut prompt, "### Constraints", &packet.constraints);
    write_st_bullets(&mut prompt, "### Decisions", &packet.decisions);
    write_st_bullets(&mut prompt, "### Open Loops", &packet.open_loops);
    write_st_bullets(&mut prompt, "### Durable Context", &durable_context);
    write_st_bullets(&mut prompt, "### Relevant Files", &packet.relevant_files);

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
    write_st_bullets(&mut prompt, "### Relevant Artifacts", &artifact_lines);
    write_st_bullets(
        &mut prompt,
        "### Retrieval Guidance",
        &packet.retrieval_guidance,
    );

    prompt.push_str("## HOW TO USE CONTYNU MEMORY\n\n");
    prompt.push_str("You have Contynu MCP tools available. Use them to manage your memory:\n\n");
    prompt.push_str("- **record_prompt**: ALWAYS call this with the user's verbatim prompt at each generation stop. If the prompt is ambiguous, include your interpretation.\n");
    prompt.push_str("- **write_memory**: Write facts, decisions, constraints, or knowledge worth recalling in future sessions. Kinds: fact, constraint, decision, todo, user_fact, project_knowledge. Scopes: user, project, session.\n");
    prompt.push_str("- **update_memory**: Correct or refine an existing memory by its ID instead of creating duplicates.\n");
    prompt.push_str("- **delete_memory**: Remove a memory that is no longer relevant.\n");
    prompt.push_str(
        "- **search_memory**: Search existing memories before writing to avoid duplicates.\n",
    );
    prompt.push_str("- **list_memories**: Browse all active memories.\n");
    prompt.push_str("- **suggest_consolidation**: Scan for redundant memory clusters that can be merged into Golden Facts.\n");
    prompt.push_str("- **consolidate_memories**: Merge related memories into a single high-fidelity Golden Fact. Originals are superseded, not deleted.\n\n");
    prompt.push_str("Carry this continuity forward naturally. If the user asks about prior work, answer from this memory instead of claiming there is no earlier context.\n");
    prompt
}

fn write_st_bullets(buffer: &mut String, title: &str, items: &[String]) {
    buffer.push_str(title);
    buffer.push_str("\n\n");
    if items.is_empty() {
        buffer.push_str("- None recorded.\n\n");
        return;
    }
    for item in items {
        // Only use one_line for short, bullet-style metadata lists if needed.
        // For memory content, we want to preserve multi-line structure.
        if title.contains("Files") || title.contains("Artifacts") {
            let _ = writeln!(buffer, "- {}", one_line(item));
        } else {
            let _ = writeln!(buffer, "- {}", item.trim());
        }
    }
    buffer.push('\n');
}

fn render_launcher_structured(packet: &RehydrationPacket) -> String {
    let mut sections = Vec::new();
    sections.push(format!(
        "Continue this Contynu project with prior memory. Project: {}.",
        packet.project_id
    ));
    if !packet.mission.trim().is_empty()
        && packet.mission != "Continue the session faithfully from canonical state."
    {
        sections.push(format!("Active Mission: {}", one_line(&packet.mission)));
    }
    if !packet.current_state.is_empty() && !is_operational(&packet.current_state) {
        sections.push(format!("Last Focus: {}", one_line(&packet.current_state)));
    }

    let clean_facts: Vec<_> = packet
        .stable_facts
        .iter()
        .filter(|f| !is_operational(f))
        .map(|f| one_line(f))
        .collect();
    if !clean_facts.is_empty() {
        sections.push(format!("Key Facts: {}", clean_facts.join(" | ")));
    }

    let clean_decisions: Vec<_> = packet.decisions.iter().map(|d| one_line(d)).collect();
    if !clean_decisions.is_empty() {
        sections.push(format!("Decisions: {}", clean_decisions.join(" | ")));
    }

    let clean_loops: Vec<_> = packet.open_loops.iter().map(|l| one_line(l)).collect();
    if !clean_loops.is_empty() {
        sections.push(format!("Todos: {}", clean_loops.join(" | ")));
    }

    let dialogue: Vec<_> = packet
        .recent_verbatim_context
        .iter()
        .filter(|line| !is_operational(line))
        .map(|line| one_line(line))
        .collect();
    if !dialogue.is_empty() {
        sections.push(format!("Recent conversation: {}", dialogue.join(" | ")));
    }

    sections.push("Use GEMINI.md for full context. Do not restate it.".into());
    sections.join("\n")
}

pub(crate) fn is_operational(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("command `")
        || lower.contains(" exited with ")
        || lower.contains(" using pty transport")
        || lower.contains(" using pipes transport")
        || lower.contains(" using inherit_terminal transport")
        || lower.starts_with("last turn used `")
        || lower.starts_with("session has ")
}

// ---------------------------------------------------------------------------
// Memory Export (for OpenClaw MEMORY.md write-back)
// ---------------------------------------------------------------------------

/// Render active memories as Markdown for export, optionally wrapped in
/// HTML comment markers compatible with OpenClaw's MEMORY.md format.
pub fn render_memory_export(
    memories: &[MemoryObject],
    max_chars: usize,
    with_markers: bool,
) -> String {
    let mut out = String::new();

    if with_markers {
        out.push_str("<!-- contynu-memory-sync:start -->\n");
    }
    out.push_str("## Project Memory (synced by Contynu)\n\n");

    let kinds_and_headers = [
        (MemoryObjectKind::Fact, "Key Facts"),
        (MemoryObjectKind::Decision, "Decisions"),
        (MemoryObjectKind::Constraint, "Constraints"),
        (MemoryObjectKind::Todo, "Open Tasks"),
        (MemoryObjectKind::UserFact, "User Facts"),
        (MemoryObjectKind::ProjectKnowledge, "Project Knowledge"),
    ];

    for (kind, header) in &kinds_and_headers {
        let items: Vec<&MemoryObject> = memories
            .iter()
            .filter(|m| m.kind == *kind && !is_operational(&m.text))
            .collect();

        if items.is_empty() {
            continue;
        }

        let section_header = format!("### {header}\n");
        if out.len() + section_header.len() > max_chars.saturating_sub(100) {
            break;
        }
        out.push_str(&section_header);

        for m in &items {
            let line = format!(
                "- {} [importance: {:.2}]\n",
                one_line(&m.text),
                m.importance
            );
            if out.len() + line.len() > max_chars.saturating_sub(60) {
                break;
            }
            out.push_str(&line);
        }
        out.push('\n');
    }

    let _ = writeln!(out, "*Last synced: {}*", chrono::Utc::now().to_rfc3339());

    if with_markers {
        out.push_str("<!-- contynu-memory-sync:end -->\n");
    }

    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn dedup_lines<'a>(items: impl Iterator<Item = &'a str>, limit: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let normalized = one_line(item);
        if normalized.is_empty() || !seen.insert(normalized.clone()) {
            continue;
        }
        out.push(normalized);
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn combined_context(packet: &RehydrationPacket) -> Vec<String> {
    let iter = packet
        .stable_facts
        .iter()
        .chain(packet.user_facts.iter())
        .chain(packet.project_knowledge.iter())
        .map(|s| s.as_str());
    dedup_lines(iter, 10)
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::{RehydrationArtifact, RehydrationPacket};
    use crate::ids::ProjectId;

    fn test_packet() -> RehydrationPacket {
        RehydrationPacket {
            schema_version: 2,
            project_identity: String::new(),
            compact_brief: String::new(),
            project_id: ProjectId::parse("prj_019d503680a475a3ae465200a90cd4fa").unwrap(),
            target_model: None,
            mission: "Fix the authentication bug".into(),
            stable_facts: vec!["The API uses REST with JWT tokens.".into()],
            constraints: vec!["Must support backward compatibility.".into()],
            decisions: vec!["Use HMAC-SHA256 for token signing.".into()],
            current_state: "Auth middleware is half-refactored.".into(),
            open_loops: vec!["Token refresh endpoint not yet implemented.".into()],
            user_facts: vec!["Developer prefers explicit error handling.".into()],
            project_knowledge: vec!["Service uses PostgreSQL 15 in production.".into()],
            relevant_artifacts: vec![RehydrationArtifact {
                path: "src/auth.rs".into(),
                kind: "source".into(),
                sha256: "sha256:abc123".into(),
            }],
            relevant_files: vec!["src/main.rs".into()],
            recent_verbatim_context: vec![
                "User: Can you fix the token expiry?".into(),
                "Assistant: I've updated the JWT middleware.".into(),
            ],
            retrieval_guidance: vec!["Use the journal for exact replay.".into()],
            recent_changes: vec![
                "Latest user request: Can you fix the token expiry?".into(),
                "Decision: Use HMAC-SHA256 for token signing.".into(),
            ],
            first_run: false,
            memory_provenance: Vec::new(),
        }
    }

    #[test]
    fn rendered_prompt_is_capped_and_points_to_mcp_fallback() {
        let mut packet = test_packet();
        // Inflate the packet past the hard ceiling so the cap must kick in.
        packet.stable_facts = (0..5_000)
            .map(|i| format!("Durable fact #{i}: the service remembers everything across runs."))
            .collect();
        let output = render_rehydration(&packet, PromptFormat::Xml, "claude_cli");
        assert!(output.len() <= MAX_RENDERED_PROMPT_BYTES);
        assert!(output.contains("contynu: context truncated"));
        assert!(output.contains("search_memory"));
    }

    #[test]
    fn rendered_prompt_is_unchanged_when_under_cap() {
        let packet = test_packet();
        let output = render_rehydration(&packet, PromptFormat::Xml, "claude_cli");
        assert!(output.len() < MAX_RENDERED_PROMPT_BYTES);
        assert!(!output.contains("contynu: context truncated"));
    }

    #[test]
    fn xml_format_produces_valid_xml_structure() {
        let packet = test_packet();
        let output = render_rehydration(&packet, PromptFormat::Xml, "claude_cli");
        assert!(output.contains("<contynu_memory"));
        assert!(output.contains("</contynu_memory>"));
        assert!(output.contains("<mission>Fix the authentication bug</mission>"));
        assert!(output.contains("<facts>"));
        assert!(output.contains("<turn role=\"user\">"));
        assert!(output.contains("<artifact kind=\"source\""));
    }

    #[test]
    fn markdown_format_produces_headers_and_bullets() {
        let packet = test_packet();
        let output = render_rehydration(&packet, PromptFormat::Markdown, "codex_cli");
        assert!(output.contains("# Contynu Working Continuation"));
        assert!(output.contains("## Current Goal"));
        assert!(output.contains("## What Changed"));
        assert!(output.contains("## Open Threads"));
        assert!(output.contains("## Durable Context"));
        assert!(output.contains("## Contynu Usage"));
    }

    #[test]
    fn codex_markdown_shows_first_run_guidance() {
        let mut packet = test_packet();
        packet.first_run = true;
        packet.recent_changes.clear();
        packet.recent_verbatim_context.clear();
        let output = render_rehydration(&packet, PromptFormat::Markdown, "codex_cli");
        assert!(output.contains("## Startup Mode"));
        assert!(output.contains("no prior Contynu memory"));
    }

    #[test]
    fn generic_markdown_format_still_produces_full_sections() {
        let packet = test_packet();
        let output = render_rehydration(&packet, PromptFormat::Markdown, "futurellm");
        assert!(output.contains("# Contynu Memory Context"));
        assert!(output.contains("## Mission"));
        assert!(output.contains("## Durable Context"));
        assert!(output.contains("> **User:**"));
    }

    #[test]
    fn structured_text_deduplicates_durable_context() {
        let mut packet = test_packet();
        packet.user_facts = vec!["Service uses PostgreSQL 15 in production.".into()];
        let output = render_rehydration(&packet, PromptFormat::StructuredText, "gemini_cli");
        assert!(output.contains("## KEY CONTEXT FROM PRIOR SESSIONS"));
        assert!(output.contains("### Durable Context"));
    }

    #[test]
    fn structured_text_matches_legacy_format() {
        let packet = test_packet();
        let output = render_rehydration(&packet, PromptFormat::StructuredText, "gemini_cli");
        assert!(output.contains("## ROLE"));
        assert!(output.contains("You are gemini_cli powered by Contynu"));
        assert!(output.contains("### Mission"));
        assert!(output.contains("### Durable Context"));
        assert!(output.contains("Carry this continuity forward naturally."));
    }

    #[test]
    fn launcher_prompts_are_compact() {
        let packet = test_packet();
        for format in [
            PromptFormat::Xml,
            PromptFormat::Markdown,
            PromptFormat::StructuredText,
        ] {
            let output = render_launcher(&packet, format);
            assert!(
                output.len() < 1000,
                "Launcher prompt too long for {:?}: {} chars",
                format,
                output.len()
            );
            assert!(output.contains(&packet.project_id.to_string()));
        }
    }
}
