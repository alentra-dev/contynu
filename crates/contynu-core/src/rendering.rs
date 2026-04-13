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

/// Render a full rehydration prompt in the given format.
pub fn render_rehydration(
    packet: &RehydrationPacket,
    format: PromptFormat,
    adapter_name: &str,
) -> String {
    match format {
        PromptFormat::Xml => render_xml(packet, adapter_name),
        PromptFormat::Markdown => render_markdown(packet, adapter_name),
        PromptFormat::StructuredText => render_structured_text(packet, adapter_name),
    }
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
    let _ = writeln!(
        out,
        "<contynu_memory project=\"{}\" schema=\"{}\" adapter=\"{}\">",
        packet.project_id, packet.schema_version, adapter_name
    );

    if !packet.project_identity.is_empty() {
        let _ = writeln!(out, "  <identity>{}</identity>", xml_escape(&packet.project_identity));
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

    write_xml_section(&mut out, "facts", "fact", &packet.stable_facts, &packet.memory_provenance);
    write_xml_section(&mut out, "constraints", "constraint", &packet.constraints, &packet.memory_provenance);
    write_xml_section(&mut out, "decisions", "decision", &packet.decisions, &packet.memory_provenance);
    write_xml_section(&mut out, "open_loops", "todo", &packet.open_loops, &packet.memory_provenance);

    if !packet.recent_verbatim_context.is_empty() {
        out.push_str("  <recent_dialogue>\n");
        for line in &packet.recent_verbatim_context {
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

    if !packet.retrieval_guidance.is_empty() {
        out.push_str("  <guidance>\n");
        for item in &packet.retrieval_guidance {
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
    out.push_str("    - search_memory: Search existing memories before writing to avoid duplicates.\n");
    out.push_str("    - list_memories: Browse all active memories.\n");
    out.push_str("    Carry this continuity forward naturally. If the user asks about prior work, answer from this memory instead of claiming there is no earlier context.\n");
    out.push_str("  </instruction>\n");
    out.push_str("</contynu_memory>\n");
    out
}

fn write_xml_section(
    out: &mut String,
    section: &str,
    item_tag: &str,
    items: &[String],
    provenance: &[crate::checkpoint::MemoryProvenance],
) {
    let _ = writeln!(out, "  <{section}>");
    if items.is_empty() {
        let _ = writeln!(out, "    <{item_tag}>None recorded.</{item_tag}>");
    } else {
        for (i, item) in items.iter().enumerate() {
            let source_attr = provenance
                .iter()
                .filter(|p| p.kind == item_tag || (item_tag == "todo" && p.kind == "todo") || (item_tag == "fact" && p.kind == "fact"))
                .nth(i)
                .and_then(|p| p.source_model.as_deref())
                .unwrap_or("unknown");
            let _ = writeln!(
                out,
                "    <{item_tag} source=\"{source_attr}\">{}</{item_tag}>",
                xml_escape(item)
            );
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
        let _ = write!(out, "<mission>{}</mission>", xml_escape(&one_line(&packet.mission)));
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

    write_md_section(&mut out, "Stable Facts", &packet.stable_facts);
    write_md_section(&mut out, "Constraints", &packet.constraints);
    write_md_section(&mut out, "Decisions", &packet.decisions);
    write_md_section(&mut out, "Open Loops", &packet.open_loops);

    if !packet.recent_verbatim_context.is_empty() {
        out.push_str("## Recent Dialogue\n\n");
        for line in &packet.recent_verbatim_context {
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
    out.push_str("- **search_memory**: Search existing memories before writing to avoid duplicates.\n");
    out.push_str("- **list_memories**: Browse all active memories.\n\n");
    out.push_str("*Carry this continuity forward naturally. If the user asks about prior work, answer from this memory instead of claiming there is no earlier context.*\n");
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
    sections.push(
        "Use this as prior context, but do not restate it unless relevant.".into(),
    );
    sections.join(" ")
}

// ---------------------------------------------------------------------------
// Structured text rendering (Gemini / generic fallback)
// ---------------------------------------------------------------------------

fn render_structured_text(packet: &RehydrationPacket, adapter_name: &str) -> String {
    let mut prompt = String::new();

    // Quick-reference summary at the top so the model sees key facts immediately.
    prompt.push_str("IMPORTANT: This file contains project memory from prior sessions. READ THIS FIRST before searching files.\n\n");
    if !packet.project_identity.is_empty() {
        let _ = writeln!(prompt, "{}\n", packet.project_identity);
    }
    if !packet.compact_brief.is_empty() {
        prompt.push_str("QUICK BRIEF:\n");
        for line in packet.compact_brief.lines() {
            let _ = writeln!(prompt, "  {}", line);
        }
        prompt.push('\n');
    }
    if !packet.stable_facts.is_empty() {
        prompt.push_str("KEY FACTS FROM PRIOR SESSIONS:\n");
        for fact in &packet.stable_facts {
            if !is_operational(fact) {
                let _ = writeln!(prompt, "  * {}", one_line(fact));
            }
        }
        prompt.push('\n');
    }

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

    write_st_bullets(
        &mut prompt,
        "Recent Conversation",
        &packet.recent_verbatim_context,
    );
    write_st_bullets(&mut prompt, "Stable Facts", &packet.stable_facts);
    write_st_bullets(&mut prompt, "Constraints", &packet.constraints);
    write_st_bullets(&mut prompt, "Decisions", &packet.decisions);
    write_st_bullets(&mut prompt, "Open Loops", &packet.open_loops);
    write_st_bullets(&mut prompt, "Relevant Files", &packet.relevant_files);

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
    write_st_bullets(&mut prompt, "Relevant Artifacts", &artifact_lines);
    write_st_bullets(
        &mut prompt,
        "Retrieval Guidance",
        &packet.retrieval_guidance,
    );

    prompt.push_str("HOW TO USE CONTYNU MEMORY\n\n");
    prompt.push_str("You have Contynu MCP tools available. Use them to manage your memory:\n");
    prompt.push_str("- record_prompt: ALWAYS call this with the user's verbatim prompt at each generation stop. If the prompt is ambiguous, include your interpretation.\n");
    prompt.push_str("- write_memory: Write facts, decisions, constraints, or knowledge worth recalling in future sessions. You decide what is worth remembering from your own output. Kinds: fact, constraint, decision, todo, user_fact, project_knowledge. Scopes: user (follows the user everywhere), project (this project only), session (ephemeral).\n");
    prompt.push_str("- update_memory: Correct or refine an existing memory by its ID instead of creating duplicates.\n");
    prompt.push_str("- delete_memory: Remove a memory that is no longer relevant.\n");
    prompt.push_str("- search_memory: Search existing memories before writing to avoid duplicates.\n");
    prompt.push_str("- list_memories: Browse all active memories.\n\n");
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
        let _ = writeln!(buffer, "- {}", one_line(item));
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
        sections.push(format!("Mission: {}", one_line(&packet.mission)));
    }
    if !packet.current_state.is_empty() && !is_operational(&packet.current_state) {
        sections.push(format!("Current focus: {}", one_line(&packet.current_state)));
    }

    let clean_facts: Vec<_> = packet.stable_facts.iter()
        .filter(|f| !is_operational(f))
        .map(|f| one_line(f))
        .collect();
    if !clean_facts.is_empty() {
        sections.push(format!("Stable facts: {}", clean_facts.join(" | ")));
    }

    let clean_decisions: Vec<_> = packet.decisions.iter()
        .map(|d| one_line(d))
        .collect();
    if !clean_decisions.is_empty() {
        sections.push(format!("Decisions: {}", clean_decisions.join(" | ")));
    }

    let clean_loops: Vec<_> = packet.open_loops.iter()
        .map(|l| one_line(l))
        .collect();
    if !clean_loops.is_empty() {
        sections.push(format!("Open loops: {}", clean_loops.join(" | ")));
    }

    let dialogue: Vec<_> = packet.recent_verbatim_context.iter()
        .filter(|line| !is_operational(line))
        .map(|line| one_line(line))
        .collect();
    if !dialogue.is_empty() {
        sections.push(format!("Recent conversation: {}", dialogue.join(" | ")));
    }

    sections.push(
        "Use this as prior context, but do not restate it unless relevant. If exact history is needed, use the Contynu rehydration files from the environment."
            .into(),
    );
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
pub fn render_memory_export(memories: &[MemoryObject], max_chars: usize, with_markers: bool) -> String {
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
            let line = format!("- {} [importance: {:.2}]\n", one_line(&m.text), m.importance);
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

fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
            memory_provenance: Vec::new(),
        }
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
        assert!(output.contains("# Contynu Memory Context"));
        assert!(output.contains("## Mission"));
        assert!(output.contains("## Stable Facts"));
        assert!(output.contains("> **User:**"));
        assert!(output.contains("| source |"));
    }

    #[test]
    fn structured_text_matches_legacy_format() {
        let packet = test_packet();
        let output = render_rehydration(&packet, PromptFormat::StructuredText, "gemini_cli");
        assert!(output.contains("Contynu continuity context for gemini_cli."));
        assert!(output.contains("Mission\n"));
        assert!(output.contains("Stable Facts\n"));
        assert!(output.contains("Carry this continuity forward naturally."));
    }

    #[test]
    fn launcher_prompts_are_compact() {
        let packet = test_packet();
        for format in [PromptFormat::Xml, PromptFormat::Markdown, PromptFormat::StructuredText] {
            let output = render_launcher(&packet, format);
            assert!(output.len() < 1000, "Launcher prompt too long for {:?}: {} chars", format, output.len());
            assert!(output.contains(&packet.project_id.to_string()));
        }
    }
}
