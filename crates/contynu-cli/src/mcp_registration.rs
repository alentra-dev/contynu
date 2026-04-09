use std::path::Path;

use anyhow::Result;

/// Ensure the Contynu MCP server is registered in the target CLI's configuration.
/// Called from launch_llm before starting the LLM session.
pub fn ensure_mcp_registered(
    cli_name: &str,
    state_dir: &Path,
    cwd: &Path,
    project_id: &str,
) -> Result<()> {
    // Don't register MCP from ephemeral/temp state dirs — they produce stale
    // global configs that point to directories that no longer exist.
    if is_ephemeral_path(state_dir) {
        return Ok(());
    }

    match cli_name {
        "claude" | "claude-code" => ensure_claude_mcp(state_dir, cwd, project_id),
        "codex" | "codex-cli" => ensure_codex_mcp(state_dir, project_id),
        "gemini" | "gemini-cli" => ensure_gemini_mcp(state_dir, project_id),
        "openclaw" => ensure_openclaw_mcp(state_dir, cwd, project_id),
        _ => Ok(()), // unknown CLI, skip
    }
}

/// Returns true if a path looks ephemeral (temp dir, worktree, etc.)
/// and should not be persisted into global MCP registrations.
fn is_ephemeral_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.starts_with("/tmp/")
        || s.starts_with("/var/tmp/")
        || s.contains("/.tmp")
        || s.contains("/tmp.")
}

fn ensure_claude_mcp(state_dir: &Path, cwd: &Path, project_id: &str) -> Result<()> {
    let mcp_path = cwd.join(".mcp.json");
    let state_dir_abs = std::fs::canonicalize(state_dir).unwrap_or_else(|_| state_dir.to_path_buf());

    let entry = serde_json::json!({
        "command": "contynu",
        "args": ["mcp-server"],
        "env": {
            "CONTYNU_STATE_DIR": state_dir_abs.display().to_string(),
            "CONTYNU_ACTIVE_PROJECT": project_id
        }
    });

    // --mcp-config expects {"mcpServers": {...}} wrapper format
    let mut config: serde_json::Value = if mcp_path.exists() {
        let content = std::fs::read_to_string(&mcp_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({"mcpServers": {}}))
    } else {
        serde_json::json!({"mcpServers": {}})
    };

    // Ensure mcpServers key exists
    if !config.get("mcpServers").is_some() {
        config["mcpServers"] = serde_json::json!({});
    }

    // Always update to ensure project ID is current
    config["mcpServers"]["contynu"] = entry;
    std::fs::write(&mcp_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

fn ensure_codex_mcp(state_dir: &Path, project_id: &str) -> Result<()> {
    let home = dirs_or_home();
    let config_path = home.join(".codex").join("config.toml");
    let state_dir_abs = std::fs::canonicalize(state_dir).unwrap_or_else(|_| state_dir.to_path_buf());

    if !config_path.exists() {
        return Ok(()); // no codex config, skip
    }

    let content = std::fs::read_to_string(&config_path)?;

    // Check if contynu MCP is already registered
    if content.contains("[mcp_servers.contynu]") {
        // Update the project ID by replacing the env line
        let updated = update_codex_project_id(&content, &state_dir_abs, project_id);
        std::fs::write(&config_path, updated)?;
        return Ok(());
    }

    // Append the registration
    let block = format!(
        r#"

[mcp_servers.contynu]
command = "contynu"
args = ["mcp-server"]

[mcp_servers.contynu.env]
CONTYNU_STATE_DIR = "{}"
CONTYNU_ACTIVE_PROJECT = "{}"
"#,
        state_dir_abs.display(),
        project_id
    );

    let mut file = std::fs::OpenOptions::new().append(true).open(&config_path)?;
    std::io::Write::write_all(&mut file, block.as_bytes())?;
    Ok(())
}

fn update_codex_project_id(content: &str, state_dir: &Path, project_id: &str) -> String {
    let mut result = String::new();
    let mut in_contynu_env = false;

    for line in content.lines() {
        if line.trim() == "[mcp_servers.contynu.env]" {
            in_contynu_env = true;
            result.push_str(line);
            result.push('\n');
            continue;
        }

        if in_contynu_env {
            if line.starts_with("CONTYNU_STATE_DIR") {
                result.push_str(&format!(
                    "CONTYNU_STATE_DIR = \"{}\"",
                    state_dir.display()
                ));
                result.push('\n');
                continue;
            } else if line.starts_with("CONTYNU_ACTIVE_PROJECT") {
                result.push_str(&format!(
                    "CONTYNU_ACTIVE_PROJECT = \"{}\"",
                    project_id
                ));
                result.push('\n');
                continue;
            } else if line.starts_with('[') {
                in_contynu_env = false;
            }
        }

        result.push_str(line);
        result.push('\n');
    }
    result
}

fn ensure_gemini_mcp(state_dir: &Path, project_id: &str) -> Result<()> {
    let state_dir_abs = std::fs::canonicalize(state_dir).unwrap_or_else(|_| state_dir.to_path_buf());

    // Check if already registered
    let list_output = std::process::Command::new("gemini")
        .args(["mcp", "list"])
        .output();

    match list_output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("contynu") {
                // Already registered — update by removing and re-adding
                let _ = std::process::Command::new("gemini")
                    .args(["mcp", "remove", "contynu"])
                    .output();
            }
        }
        Err(_) => return Ok(()), // gemini not available or doesn't support mcp
    }

    // Register with env vars for state dir and active project
    let _ = std::process::Command::new("gemini")
        .args([
            "mcp", "add", "contynu",
            "contynu", "mcp-server",
            "-e", &format!("CONTYNU_STATE_DIR={}", state_dir_abs.display()),
            "-e", &format!("CONTYNU_ACTIVE_PROJECT={}", project_id),
            "--trust",
            "--scope", "user",
        ])
        .output();

    Ok(())
}

fn ensure_openclaw_mcp(state_dir: &Path, config_path: &Path, project_id: &str) -> Result<()> {
    let state_dir_abs = std::fs::canonicalize(state_dir).unwrap_or_else(|_| state_dir.to_path_buf());

    // config_path here is the OpenClaw config file path (passed as cwd from openclaw_setup)
    let oc_config = if config_path.is_file() {
        config_path.to_path_buf()
    } else {
        let home = dirs_or_home();
        home.join(".openclaw").join("openclaw.json")
    };

    if !oc_config.exists() {
        return Ok(()); // OpenClaw not installed
    }

    let content = std::fs::read_to_string(&oc_config)?;
    // Simple check — if contynu is already in the MCP section, skip
    if content.contains("\"contynu\"") && content.contains("mcp-server") {
        return Ok(());
    }

    // Parse as JSON, add MCP server entry
    let mut config: serde_json::Value = serde_json::from_str(&content)
        .unwrap_or_else(|_| serde_json::json!({}));

    if !config.get("mcp").is_some() {
        config["mcp"] = serde_json::json!({});
    }
    if !config["mcp"].get("servers").is_some() {
        config["mcp"]["servers"] = serde_json::json!({});
    }

    config["mcp"]["servers"]["contynu"] = serde_json::json!({
        "command": "contynu",
        "args": ["mcp-server"],
        "env": {
            "CONTYNU_STATE_DIR": state_dir_abs.display().to_string(),
            "CONTYNU_ACTIVE_PROJECT": project_id
        }
    });

    std::fs::write(&oc_config, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

fn dirs_or_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}
