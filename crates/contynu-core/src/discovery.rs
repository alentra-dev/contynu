//! Discovery and ingestion of external AI tool sessions.
//!
//! Scans local session/memory files from Claude Code, Codex, and Gemini,
//! checks if they've already been ingested into Contynu, and imports
//! new memories while preserving the structure models use when writing
//! to Contynu natively.

use std::path::{Path, PathBuf};

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::ids::{MemoryId, SessionId};
use crate::store::{MemoryObject, MemoryObjectKind, MemoryScope, MetadataStore};

/// A discovered memory from an external AI tool, ready for ingestion.
#[derive(Debug, Clone)]
pub struct DiscoveredMemory {
    pub source_tool: String,
    pub source_path: String,
    pub source_key: String,
    pub text: String,
    pub kind: MemoryObjectKind,
    pub scope: MemoryScope,
    pub importance: f64,
    pub reason: String,
}

/// Result of a discovery scan.
#[derive(Debug, Default)]
pub struct DiscoveryReport {
    pub claude_memories: Vec<DiscoveredMemory>,
    pub codex_memories: Vec<DiscoveredMemory>,
    pub gemini_memories: Vec<DiscoveredMemory>,
    pub total_new: usize,
    pub total_skipped: usize,
}

/// Run discovery for all supported AI tools, scoped to the given working directory.
/// Returns discovered memories that are NOT yet ingested.
pub fn discover_all(store: &MetadataStore, cwd: &Path) -> Result<DiscoveryReport> {
    let mut report = DiscoveryReport::default();

    // Claude Code discovery
    let claude_memories = discover_claude_code(store, cwd)?;
    report.total_new += claude_memories.len();
    report.claude_memories = claude_memories;

    // Codex discovery
    let codex_memories = discover_codex(store, cwd)?;
    report.total_new += codex_memories.len();
    report.codex_memories = codex_memories;

    // Gemini CLI discovery
    let gemini_memories = discover_gemini(store, cwd)?;
    report.total_new += gemini_memories.len();
    report.gemini_memories = gemini_memories;

    Ok(report)
}

/// Ingest all discovered memories into the Contynu store for the given project.
pub fn ingest_memories(
    store: &MetadataStore,
    project_id: &SessionId,
    report: &DiscoveryReport,
) -> Result<usize> {
    let mut ingested = 0;
    let all_memories = report
        .claude_memories
        .iter()
        .chain(report.codex_memories.iter())
        .chain(report.gemini_memories.iter());

    for discovered in all_memories {
        let memory_id = MemoryId::new();
        store.insert_memory_object(&MemoryObject {
            memory_id,
            session_id: project_id.clone(),
            kind: discovered.kind,
            scope: discovered.scope,
            status: "active".into(),
            text: discovered.text.clone(),
            importance: discovered.importance,
            reason: Some(discovered.reason.clone()),
            source_model: Some(discovered.source_tool.clone()),
            superseded_by: None,
            created_at: Utc::now(),
            updated_at: None,
            access_count: 0,
            last_accessed_at: None,
        })?;

        store.mark_source_ingested(&discovered.source_key, &discovered.source_tool, 1)?;
        ingested += 1;
    }

    Ok(ingested)
}

// ---------------------------------------------------------------------------
// Claude Code discovery
// ---------------------------------------------------------------------------

/// Discover Claude Code memory files for the given working directory.
///
/// Claude Code stores memories in `~/.claude/projects/<encoded-cwd>/memory/`
/// as markdown files with YAML frontmatter containing `name`, `description`, `type`.
fn discover_claude_code(store: &MetadataStore, cwd: &Path) -> Result<Vec<DiscoveredMemory>> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Ok(Vec::new()),
    };

    let claude_projects = home.join(".claude").join("projects");
    if !claude_projects.exists() {
        return Ok(Vec::new());
    }

    // Claude Code encodes CWD by replacing '/' with '-' (e.g., /home/user/project → -home-user-project)
    let cwd_str = cwd.to_string_lossy();
    let encoded_cwd = cwd_str.replace('/', "-");

    let memory_dir = claude_projects.join(&encoded_cwd).join("memory");
    if !memory_dir.exists() {
        return Ok(Vec::new());
    }

    let mut discoveries = Vec::new();

    let entries = std::fs::read_dir(&memory_dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // Skip MEMORY.md (index file) and non-markdown files
        if !path.extension().map_or(false, |e| e == "md") {
            continue;
        }
        if path.file_name().map_or(false, |n| n == "MEMORY.md") {
            continue;
        }

        let path_str = path.to_string_lossy().to_string();

        // Parse the memory file
        if let Some(memory) = parse_claude_memory_file(&path, &path_str)? {
            if store.is_source_ingested(&memory.source_key)? {
                continue;
            }
            discoveries.push(memory);
        }
    }

    Ok(discoveries)
}

/// Parse a Claude Code memory file with YAML frontmatter.
///
/// Expected format:
/// ```text
/// ---
/// name: Memory Name
/// description: One-line description
/// type: user|feedback|project|reference
/// ---
///
/// Memory content here.
/// ```
fn parse_claude_memory_file(path: &Path, source_path: &str) -> Result<Option<DiscoveredMemory>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    // Parse YAML frontmatter
    let (frontmatter, body) = match parse_frontmatter(&content) {
        Some(parts) => parts,
        None => {
            // No frontmatter — treat whole file as project_knowledge
            let text = content.trim().to_string();
            if text.is_empty() {
                return Ok(None);
            }
            return Ok(Some(DiscoveredMemory {
                source_tool: "claude_code".into(),
                source_path: source_path.into(),
                source_key: make_source_key("claude_code", source_path, "no_frontmatter", &text),
                text,
                kind: MemoryObjectKind::ProjectKnowledge,
                scope: MemoryScope::Project,
                importance: 0.5,
                reason: "Ingested from Claude Code memory file (no frontmatter)".into(),
            }));
        }
    };

    let body = body.trim().to_string();
    if body.is_empty() {
        return Ok(None);
    }

    // Map Claude Code memory type → Contynu kind
    let memory_type = extract_frontmatter_value(&frontmatter, "type")
        .unwrap_or_default()
        .to_lowercase();
    let (kind, scope) = match memory_type.as_str() {
        "user" => (MemoryObjectKind::UserFact, MemoryScope::User),
        "feedback" => (MemoryObjectKind::Constraint, MemoryScope::Project),
        "project" => (MemoryObjectKind::ProjectKnowledge, MemoryScope::Project),
        "reference" => (MemoryObjectKind::Fact, MemoryScope::Project),
        _ => (MemoryObjectKind::ProjectKnowledge, MemoryScope::Project),
    };

    let name = extract_frontmatter_value(&frontmatter, "name").unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    });

    let importance = match memory_type.as_str() {
        "user" => 0.8,
        "feedback" => 0.7,
        "project" => 0.6,
        "reference" => 0.5,
        _ => 0.5,
    };

    Ok(Some(DiscoveredMemory {
        source_tool: "claude_code".into(),
        source_path: source_path.into(),
        source_key: make_source_key("claude_code", source_path, &memory_type, &body),
        text: body,
        kind,
        scope,
        importance,
        reason: format!("Ingested from Claude Code memory: {name}"),
    }))
}

// ---------------------------------------------------------------------------
// Codex discovery
// ---------------------------------------------------------------------------

/// Discover Codex memory files for the given working directory.
///
/// Codex stores memories in `~/.codex/memories/` as plain markdown files
/// and session history in `~/.codex/history.jsonl`.
fn discover_codex(store: &MetadataStore, cwd: &Path) -> Result<Vec<DiscoveredMemory>> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Ok(Vec::new()),
    };

    let mut discoveries = Vec::new();

    // Discover Codex memory files
    let codex_memories_dir = home.join(".codex").join("memories");
    if codex_memories_dir.exists() {
        discover_codex_memories(store, &codex_memories_dir, &mut discoveries)?;
    }

    // Discover Codex session prompts matching this CWD
    let codex_history = home.join(".codex").join("history.jsonl");
    if codex_history.exists() {
        discover_codex_prompts(store, &codex_history, cwd, &mut discoveries)?;
    }

    Ok(discoveries)
}

fn discover_codex_memories(
    store: &MetadataStore,
    memories_dir: &Path,
    discoveries: &mut Vec<DiscoveredMemory>,
) -> Result<()> {
    let entries = std::fs::read_dir(memories_dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.extension().map_or(false, |e| e == "md") {
            continue;
        }

        let path_str = path.to_string_lossy().to_string();
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let text = content.trim().to_string();
        if text.is_empty() {
            continue;
        }

        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let source_key = make_source_key("codex", &path_str, "memory_file", &text);
        if store.is_source_ingested(&source_key)? {
            continue;
        }

        discoveries.push(DiscoveredMemory {
            source_tool: "codex".into(),
            source_path: path_str,
            source_key,
            text,
            kind: MemoryObjectKind::ProjectKnowledge,
            scope: MemoryScope::Project,
            importance: 0.6,
            reason: format!("Ingested from Codex memory: {name}"),
        });
    }
    Ok(())
}

fn discover_codex_prompts(
    store: &MetadataStore,
    history_path: &Path,
    cwd: &Path,
    discoveries: &mut Vec<DiscoveredMemory>,
) -> Result<()> {
    // Codex history.jsonl contains lines like:
    // {"session_id":"...","ts":1234567890,"text":"user prompt text"}
    // We also need to check session files to match by CWD.

    let content = match std::fs::read_to_string(history_path) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    // Collect session IDs that match our CWD from session metadata files
    let matching_sessions = find_codex_sessions_by_cwd(cwd);

    for (line_idx, line) in content.lines().enumerate() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let session_id = entry
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let text = entry.get("text").and_then(|v| v.as_str()).unwrap_or("");

        if text.is_empty() || session_id.is_empty() {
            continue;
        }

        // Only ingest prompts from sessions that match our working directory
        if !matching_sessions.is_empty() && !matching_sessions.contains(&session_id.to_string()) {
            continue;
        }

        // Only take substantive prompts (skip very short ones)
        if text.len() < 20 {
            continue;
        }

        let source_path = history_path.to_string_lossy().to_string();
        let source_key = make_source_key(
            "codex",
            &source_path,
            &format!("history:{session_id}:{line_idx}"),
            text,
        );
        if store.is_source_ingested(&source_key)? {
            continue;
        }

        discoveries.push(DiscoveredMemory {
            source_tool: "codex".into(),
            source_path,
            source_key,
            text: format!("[Codex session prompt] {text}"),
            kind: infer_kind_from_text(text),
            scope: MemoryScope::Project,
            importance: infer_importance_from_text(text),
            reason: format!("Ingested from Codex history session {session_id}"),
        });
    }

    Ok(())
}

/// Find Codex session IDs that were started in the given CWD.
fn find_codex_sessions_by_cwd(cwd: &Path) -> Vec<String> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let sessions_dir = home.join(".codex").join("sessions");
    if !sessions_dir.exists() {
        return Vec::new();
    }

    let cwd_str = cwd.to_string_lossy().to_string();
    let mut matching = Vec::new();

    // Walk session JSONL files and check their CWD from session_meta
    if let Ok(walker) = walk_jsonl_files(&sessions_dir) {
        for path in walker {
            if let Ok(first_line) = read_first_line(&path) {
                if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&first_line) {
                    if meta.get("type").and_then(|v| v.as_str()) == Some("session_meta") {
                        if let Some(payload_cwd) = meta
                            .get("payload")
                            .and_then(|p| p.get("cwd"))
                            .and_then(|v| v.as_str())
                        {
                            if payload_cwd == cwd_str {
                                if let Some(id) = meta
                                    .get("payload")
                                    .and_then(|p| p.get("id"))
                                    .and_then(|v| v.as_str())
                                {
                                    matching.push(id.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    matching
}

// ---------------------------------------------------------------------------
// Gemini CLI discovery
// ---------------------------------------------------------------------------

/// Discover Gemini CLI memory for the given working directory.
///
/// Gemini stores memories in two places:
/// - `~/.gemini/GEMINI.md` — global memory (bullet points under "## Gemini Added Memories")
/// - `<project_root>/GEMINI.md` — per-project memory file
///
/// Project mapping is in `~/.gemini/projects.json` which maps CWD → project slug.
/// Session history dirs live at `~/.gemini/history/<slug>/` with `.project_root` files.
fn discover_gemini(store: &MetadataStore, cwd: &Path) -> Result<Vec<DiscoveredMemory>> {
    let mut discoveries = Vec::new();

    // 1. Check for per-project GEMINI.md in the working directory
    let project_gemini_md = cwd.join("GEMINI.md");
    if project_gemini_md.exists() {
        let path_str = project_gemini_md.to_string_lossy().to_string();
        if let Ok(content) = std::fs::read_to_string(&project_gemini_md) {
            parse_gemini_md(store, &content, &path_str, "gemini", &mut discoveries)?;
        }
    }

    // 2. Check global ~/.gemini/GEMINI.md
    let home = match home_dir() {
        Some(h) => h,
        None => return Ok(discoveries),
    };

    let global_gemini_md = home.join(".gemini").join("GEMINI.md");
    if global_gemini_md.exists() {
        let path_str = global_gemini_md.to_string_lossy().to_string();
        if let Ok(content) = std::fs::read_to_string(&global_gemini_md) {
            parse_gemini_md(store, &content, &path_str, "gemini", &mut discoveries)?;
        }
    }

    Ok(discoveries)
}

/// Parse a GEMINI.md file and extract individual memories from bullet points.
///
/// Gemini uses a simple format:
/// ```text
/// ## Gemini Added Memories
/// - Keith is the name of the 1st son of Matt.
/// - Some other fact.
///
/// ## Project Goal
/// Autonomous handyman business.
///
/// ## Infrastructure
/// - Brain: Ryzen 9 Server
/// ```
///
/// Each heading becomes context, and bullet points under it become individual memories.
fn parse_gemini_md(
    store: &MetadataStore,
    content: &str,
    source_path: &str,
    source_tool: &str,
    discoveries: &mut Vec<DiscoveredMemory>,
) -> Result<()> {
    let mut current_section = String::new();
    let mut section_bullets: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("## ") || trimmed.starts_with("# ") {
            // Flush previous section
            flush_gemini_section(
                store,
                &current_section,
                &section_bullets,
                source_path,
                source_tool,
                discoveries,
            )?;
            current_section = trimmed.trim_start_matches('#').trim().to_string();
            section_bullets.clear();
        } else if trimmed.starts_with("- ") {
            let bullet = trimmed[2..].trim().to_string();
            if !bullet.is_empty() {
                section_bullets.push(bullet);
            }
        } else if !trimmed.is_empty() && !current_section.is_empty() {
            // Non-bullet text under a heading — treat as a single knowledge item
            section_bullets.push(trimmed.to_string());
        }
    }

    // Flush last section
    flush_gemini_section(
        store,
        &current_section,
        &section_bullets,
        source_path,
        source_tool,
        discoveries,
    )?;
    Ok(())
}

fn flush_gemini_section(
    store: &MetadataStore,
    section: &str,
    bullets: &[String],
    source_path: &str,
    source_tool: &str,
    discoveries: &mut Vec<DiscoveredMemory>,
) -> Result<()> {
    if bullets.is_empty() {
        return Ok(());
    }

    let section_lower = section.to_lowercase();

    // Map section headings to appropriate memory kinds
    let (kind, importance) = if section_lower.contains("memor") {
        // "Gemini Added Memories" or similar
        (MemoryObjectKind::Fact, 0.7)
    } else if section_lower.contains("goal") || section_lower.contains("mission") {
        (MemoryObjectKind::ProjectKnowledge, 0.8)
    } else if section_lower.contains("constraint")
        || section_lower.contains("rule")
        || section_lower.contains("policy")
    {
        (MemoryObjectKind::Constraint, 0.7)
    } else if section_lower.contains("decision") {
        (MemoryObjectKind::Decision, 0.7)
    } else if section_lower.contains("todo") || section_lower.contains("task") {
        (MemoryObjectKind::Todo, 0.6)
    } else {
        // Default: infrastructure, architecture, etc. → project knowledge
        (MemoryObjectKind::ProjectKnowledge, 0.6)
    };

    for bullet in bullets {
        let text = if section.is_empty() {
            bullet.clone()
        } else {
            format!("[{section}] {bullet}")
        };
        let source_key = make_source_key(source_tool, source_path, section, &text);
        if store.is_source_ingested(&source_key)? {
            continue;
        }
        discoveries.push(DiscoveredMemory {
            source_tool: source_tool.into(),
            source_path: source_path.into(),
            source_key,
            text,
            kind,
            scope: MemoryScope::Project,
            importance,
            reason: format!("Ingested from Gemini GEMINI.md: {section}"),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

fn make_source_key(tool: &str, source_path: &str, discriminator: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tool.as_bytes());
    hasher.update(b"\n");
    hasher.update(source_path.as_bytes());
    hasher.update(b"\n");
    hasher.update(discriminator.as_bytes());
    hasher.update(b"\n");
    hasher.update(text.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("{tool}:{source_path}:{discriminator}:{digest}")
}

fn infer_kind_from_text(text: &str) -> MemoryObjectKind {
    let lower = text.to_ascii_lowercase();
    if lower.contains("must ")
        || lower.contains("mustn't")
        || lower.contains("do not")
        || lower.contains("don't ")
        || lower.contains("should not")
        || lower.contains("never ")
    {
        MemoryObjectKind::Constraint
    } else if lower.starts_with("fix ")
        || lower.starts_with("implement ")
        || lower.starts_with("add ")
        || lower.starts_with("update ")
        || lower.starts_with("refactor ")
        || lower.starts_with("investigate ")
    {
        MemoryObjectKind::Todo
    } else {
        MemoryObjectKind::ProjectKnowledge
    }
}

fn infer_importance_from_text(text: &str) -> f64 {
    match infer_kind_from_text(text) {
        MemoryObjectKind::Constraint => 0.75,
        MemoryObjectKind::Todo => 0.68,
        _ => {
            if text.len() > 120 {
                0.62
            } else {
                0.56
            }
        }
    }
}

/// Parse YAML frontmatter delimited by `---`.
/// Returns (frontmatter_text, body_text) if found.
fn parse_frontmatter(content: &str) -> Option<(String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }

    let after_first = &trimmed[3..];
    let end_pos = after_first.find("\n---")?;
    let frontmatter = after_first[..end_pos].trim().to_string();
    let body = after_first[end_pos + 4..].to_string();
    Some((frontmatter, body))
}

/// Extract a value from simple YAML frontmatter (key: value format).
fn extract_frontmatter_value(frontmatter: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&prefix) {
            let value = trimmed[prefix.len()..].trim();
            // Strip surrounding quotes if present
            let value = value.trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Walk a directory recursively and collect all .jsonl file paths.
fn walk_jsonl_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    walk_jsonl_recursive(dir, &mut files)?;
    Ok(files)
}

fn walk_jsonl_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_jsonl_recursive(&path, files)?;
        } else if path.extension().map_or(false, |e| e == "jsonl") {
            files.push(path);
        }
    }
    Ok(())
}

/// Read the first line of a file.
fn read_first_line(path: &Path) -> std::io::Result<String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StatePaths;
    use crate::store::MetadataStore;
    use tempfile::tempdir;

    #[test]
    fn parse_frontmatter_extracts_yaml() {
        let content = "---\nname: Test\ntype: user\n---\n\nBody content here.";
        let (fm, body) = parse_frontmatter(content).unwrap();
        assert!(fm.contains("name: Test"));
        assert!(fm.contains("type: user"));
        assert!(body.trim() == "Body content here.");
    }

    #[test]
    fn parse_frontmatter_returns_none_without_markers() {
        assert!(parse_frontmatter("No frontmatter here").is_none());
    }

    #[test]
    fn extract_frontmatter_value_works() {
        let fm = "name: Test Memory\ntype: project\ndescription: A test";
        assert_eq!(
            extract_frontmatter_value(fm, "name"),
            Some("Test Memory".into())
        );
        assert_eq!(
            extract_frontmatter_value(fm, "type"),
            Some("project".into())
        );
        assert_eq!(extract_frontmatter_value(fm, "missing"), None);
    }

    #[test]
    fn extract_frontmatter_handles_quoted_values() {
        let fm = "name: \"Quoted Value\"\ntype: 'single quoted'";
        assert_eq!(
            extract_frontmatter_value(fm, "name"),
            Some("Quoted Value".into())
        );
        assert_eq!(
            extract_frontmatter_value(fm, "type"),
            Some("single quoted".into())
        );
    }

    #[test]
    fn claude_type_to_contynu_kind_mapping() {
        // user → user_fact
        // feedback → constraint
        // project → project_knowledge
        // reference → fact
        let mappings = [
            ("user", MemoryObjectKind::UserFact),
            ("feedback", MemoryObjectKind::Constraint),
            ("project", MemoryObjectKind::ProjectKnowledge),
            ("reference", MemoryObjectKind::Fact),
        ];
        for (claude_type, expected_kind) in mappings {
            let kind = match claude_type {
                "user" => MemoryObjectKind::UserFact,
                "feedback" => MemoryObjectKind::Constraint,
                "project" => MemoryObjectKind::ProjectKnowledge,
                "reference" => MemoryObjectKind::Fact,
                _ => MemoryObjectKind::ProjectKnowledge,
            };
            assert_eq!(kind, expected_kind, "Failed for type: {claude_type}");
        }
    }

    #[test]
    fn source_keys_change_when_content_changes() {
        let a = make_source_key("gemini", "/tmp/GEMINI.md", "Section", "First fact");
        let b = make_source_key("gemini", "/tmp/GEMINI.md", "Section", "Second fact");
        assert_ne!(a, b);
    }

    #[test]
    fn infer_codex_prompt_kind_prefers_constraint_and_todo_signals() {
        assert_eq!(
            infer_kind_from_text("Do not remove the authentication guard"),
            MemoryObjectKind::Constraint
        );
        assert_eq!(
            infer_kind_from_text("Implement the token refresh endpoint"),
            MemoryObjectKind::Todo
        );
        assert_eq!(
            infer_kind_from_text("Summarize the current architecture"),
            MemoryObjectKind::ProjectKnowledge
        );
    }

    #[test]
    fn gemini_item_level_ingestion_skips_only_ingested_bullets() {
        let dir = tempdir().unwrap();
        let state = StatePaths::new(dir.path().join(".contynu"));
        state.ensure_layout().unwrap();
        let store = MetadataStore::open(state.sqlite_db()).unwrap();

        let first_text = "[Gemini Added Memories] Keep the auth bug context";
        let first_key = make_source_key(
            "gemini",
            "/tmp/GEMINI.md",
            "Gemini Added Memories",
            first_text,
        );
        store.mark_source_ingested(&first_key, "gemini", 1).unwrap();

        let mut discoveries = Vec::new();
        parse_gemini_md(
            &store,
            "## Gemini Added Memories\n- Keep the auth bug context\n- Remember the deployment window\n",
            "/tmp/GEMINI.md",
            "gemini",
            &mut discoveries,
        )
        .unwrap();

        assert_eq!(discoveries.len(), 1);
        assert!(discoveries[0]
            .text
            .contains("Remember the deployment window"));
    }
}
