use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::{anyhow, Result};
use contynu_core::mcp::{JsonRpcRequest, McpDispatcher};
use contynu_core::{MetadataStore, ProjectId, StatePaths};

pub fn run(state_dir: &Path) -> Result<()> {
    let state = StatePaths::new(state_dir);
    let active_project = resolve_active_project(&state)?;

    let dispatcher = McpDispatcher::new(state_dir, active_project)
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

        if let Some(response) = dispatcher.handle_request(&request) {
            write_response(&mut stdout, &response)?;
        }
    }

    Ok(())
}

fn resolve_active_project(state: &StatePaths) -> Result<ProjectId> {
    // First try env var (set by contynu when launching an LLM)
    if let Ok(id) = std::env::var("CONTYNU_ACTIVE_PROJECT") {
        return ProjectId::parse(id).map_err(|e| anyhow!("Invalid CONTYNU_ACTIVE_PROJECT: {e}"));
    }

    // Fall back to primary project in the store
    if state.sqlite_db().exists() {
        let store = MetadataStore::open_readonly(state.sqlite_db())?;
        if let Some(id) = store.primary_project_id()? {
            return Ok(id);
        }
    }

    Err(anyhow!(
        "No active project. Set CONTYNU_ACTIVE_PROJECT or ensure a primary project exists."
    ))
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
