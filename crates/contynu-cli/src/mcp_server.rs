use std::io::{self, BufRead, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use anyhow::{anyhow, Result};
use contynu_core::mcp::{JsonRpcRequest, JsonRpcResponse, McpDispatcher};
use contynu_core::{MetadataStore, ProjectId, StatePaths};

pub fn run(state_dir: &Path) -> Result<()> {
    // Use the provided state_dir if it contains a valid database,
    // otherwise try to discover one from CWD.
    let effective_dir = if state_dir.join("sqlite").join("contynu.db").exists() {
        state_dir.to_path_buf()
    } else if let Some(discovered) = discover_state_dir() {
        discovered
    } else {
        state_dir.to_path_buf()
    };
    let state = StatePaths::new(&effective_dir);
    let active_project = resolve_active_project(&state)?;

    // Auto-ingest unrecorded sessions from external AI tools
    run_auto_ingestion(&state, &active_project);

    let dispatcher = McpDispatcher::new(&effective_dir, active_project)
        .map_err(|e| anyhow!("Failed to start MCP server: {e}"))?;

    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();

    for line in stdin.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = contynu_core::mcp::JsonRpcResponse::parse_error(&e.to_string());
                write_response(&mut stdout, &resp)?;
                continue;
            }
        };

        // Catch panics from any handler so a single bad request never kills
        // the server and tears down the stdio transport. The agent on the
        // other end relies on this stream staying open across many calls.
        let request_id = request.id.clone();
        let result = panic::catch_unwind(AssertUnwindSafe(|| dispatcher.handle_request(&request)));
        match result {
            Ok(Some(response)) => write_response(&mut stdout, &response)?,
            Ok(None) => {} // notification, no response expected
            Err(panic_payload) => {
                let msg = panic_message(&panic_payload);
                let response = JsonRpcResponse::err(
                    request_id,
                    -32603,
                    format!("Internal error: handler panicked: {msg}"),
                );
                write_response(&mut stdout, &response)?;
            }
        }
    }

    Ok(())
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn resolve_active_project(state: &StatePaths) -> Result<ProjectId> {
    // First try env var (set by contynu when launching an LLM)
    if let Ok(id) = std::env::var("CONTYNU_ACTIVE_PROJECT") {
        return ProjectId::parse(id).map_err(|e| anyhow!("Invalid CONTYNU_ACTIVE_PROJECT: {e}"));
    }

    // Fall back to primary project in the store
    if state.sqlite_db().exists() {
        let store = MetadataStore::open(state.sqlite_db())?;
        if let Some(id) = store.primary_project_id()? {
            return Ok(id);
        }
    }

    Err(anyhow!(
        "No active project. Set CONTYNU_ACTIVE_PROJECT or ensure a primary project exists."
    ))
}

/// Walk up from CWD looking for a `.contynu/sqlite/contynu.db` to find the
/// real project state dir. This handles cases where CONTYNU_STATE_DIR was set
/// from an ephemeral context (temp dir, worktree) that no longer exists.
fn discover_state_dir() -> Option<std::path::PathBuf> {
    let cwd = std::env::var("CONTYNU_CWD")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::current_dir())
        .ok()?;
    let mut dir = cwd.as_path();
    loop {
        let candidate = dir.join(".contynu");
        if candidate.join("sqlite").join("contynu.db").exists() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
}

/// Run auto-ingestion of external AI tool sessions on MCP server startup.
/// Non-fatal: failures are logged to stderr and don't prevent server operation.
fn run_auto_ingestion(state: &StatePaths, project_id: &ProjectId) {
    let cwd = std::env::var("CONTYNU_CWD")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::current_dir())
        .unwrap_or_default();

    if cwd.as_os_str().is_empty() {
        return;
    }

    let cwd_abs = std::fs::canonicalize(&cwd).unwrap_or(cwd);

    let store = match MetadataStore::open(state.sqlite_db()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[contynu] Auto-ingestion: failed to open store: {e}");
            return;
        }
    };

    match contynu_core::discovery::discover_all(&store, &cwd_abs) {
        Ok(report) if report.total_new > 0 => {
            match contynu_core::discovery::ingest_memories(&store, project_id, &report) {
                Ok(count) => {
                    eprintln!("[contynu] Auto-ingested {count} memories from external AI tools");
                }
                Err(e) => {
                    eprintln!("[contynu] Auto-ingestion failed: {e}");
                }
            }
        }
        Ok(_) => {} // nothing new to ingest
        Err(e) => {
            eprintln!("[contynu] Auto-discovery failed: {e}");
        }
    }
}

fn write_response(
    stdout: &mut io::StdoutLock<'_>,
    response: &contynu_core::mcp::JsonRpcResponse,
) -> Result<()> {
    let json = serde_json::to_string(response)?;
    stdout.write_all(json.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}
