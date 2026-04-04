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
    match cli_name {
        "claude" | "claude-code" => ensure_claude_mcp(state_dir, cwd, project_id),
        "codex" | "codex-cli" => ensure_codex_mcp(state_dir, project_id),
        "gemini" | "gemini-cli" => ensure_gemini_mcp(state_dir, project_id),
        _ => Ok(()), // unknown CLI, skip
    }
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

    let mut config: serde_json::Value = if mcp_path.exists() {
        let content = std::fs::read_to_string(&mcp_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Always update to ensure project ID is current
    config["contynu"] = entry;
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
                // Already registered — Gemini doesn't support updating env vars
                // in existing MCP servers, so we leave it as-is
                return Ok(());
            }
        }
        Err(_) => return Ok(()), // gemini not available or doesn't support mcp
    }

    // Register
    let _ = std::process::Command::new("gemini")
        .args([
            "mcp",
            "add",
            "contynu",
            "--",
            "contynu",
            "mcp-server",
            "--state-dir",
            &state_dir_abs.display().to_string(),
        ])
        .env("CONTYNU_ACTIVE_PROJECT", project_id)
        .output();

    Ok(())
}

fn dirs_or_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}
