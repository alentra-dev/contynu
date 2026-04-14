use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::{env, fs};

use tempfile::tempdir;

fn extract_prefixed_id(output: &str, prefix: &str) -> String {
    output
        .split_whitespace()
        .find(|token| token.starts_with(prefix))
        .unwrap_or_else(|| panic!("missing {prefix} in output: {output}"))
        .to_string()
}

fn contynu_cmd() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_contynu"));
    cmd.env("CONTYNU_SKIP_UPDATE_CHECK", "1");
    cmd
}

fn current_release_asset_name() -> &'static str {
    match (env::consts::OS, env::consts::ARCH) {
        ("linux", "x86_64") => "contynu-linux-x86_64.tar.gz",
        ("linux", "aarch64") => "contynu-linux-aarch64.tar.gz",
        ("macos", "x86_64") => "contynu-macos-x86_64.tar.gz",
        ("macos", "aarch64") => "contynu-macos-aarch64.tar.gz",
        ("windows", "x86_64") => "contynu-windows-x86_64.zip",
        ("windows", "aarch64") => "contynu-windows-aarch64.zip",
        _ => panic!("unsupported platform for smoke test"),
    }
}

fn current_installer_asset() -> &'static str {
    if env::consts::OS == "windows" {
        "install.ps1"
    } else {
        "install.sh"
    }
}

fn spawn_release_server(tag: &str, installer_body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let tag = tag.to_string();
    let asset = current_release_asset_name().to_string();
    let installer_asset = current_installer_asset().to_string();
    thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0_u8; 4096];
            let read = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let (status, body, content_type) = if path == "/latest" {
                (
                    "200 OK",
                    format!(
                        "{{\"tag_name\":\"{}\",\"assets\":[{{\"name\":\"{}\"}},{{\"name\":\"{}\"}}]}}",
                        tag, asset, installer_asset
                    ),
                    "application/json",
                )
            } else if path == format!("/downloads/{tag}/{installer_asset}") {
                ("200 OK", installer_body.clone(), "text/plain")
            } else {
                ("404 Not Found", "missing".to_string(), "text/plain")
            };
            write_http_response(&mut stream, status, content_type, &body);
        }
    });
    format!("http://{}", addr)
}

fn write_http_response(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).unwrap();
}

#[test]
fn init_and_doctor_work() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");

    let init = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("init")
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config = fs::read_to_string(state_dir.join("config.json")).unwrap();
    assert!(config.contains("\"command\": \"codex\""));
    assert!(config.contains("\"command\": \"claude\""));
    assert!(config.contains("\"command\": \"gemini\""));

    let doctor = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("doctor")
        .output()
        .unwrap();
    assert!(
        doctor.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(stdout.contains("Contynu doctor"));
    assert!(stdout.contains("State root:"));
}

#[test]
fn version_flag_reports_current_release() {
    let output = contynu_cmd().arg("--version").output().unwrap();
    assert!(
        output.status.success(),
        "--version failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("contynu 0.5.2"));
}

#[test]
fn project_is_created_and_reused_by_default() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");

    let start = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("start-project")
        .output()
        .unwrap();
    assert!(
        start.status.success(),
        "start-project failed: {}",
        String::from_utf8_lossy(&start.stderr)
    );
    let project_id = extract_prefixed_id(&String::from_utf8_lossy(&start.stdout), "prj_");
    assert!(project_id.starts_with("prj_"));

    let run = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--cwd")
        .arg(dir.path())
        .arg("run")
        .arg("--")
        .arg("bash")
        .arg("-lc")
        .arg("printf smoke")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let run_stdout = String::from_utf8_lossy(&run.stdout);
    let run_stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run_stdout.contains("smoke"));
    assert!(run_stderr.contains("Let's contynu another time."));
    assert!(run_stderr.contains("Project prj_"));

    // Verify session exists via status command
    let status = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("status")
        .output()
        .unwrap();
    assert!(status.status.success());
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status_stdout.contains(&project_id));
}

#[test]
fn streamlined_launcher_reuses_primary_project() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let codex_path = bin_dir.join("codex");
    let capture_path = dir.path().join("codex-capture.txt");
    let agents_capture_path = dir.path().join("agents-capture.txt");
    fs::write(
        &codex_path,
        format!(
            "#!/bin/sh\nprintf \"env:%s|prompt:%s\\n\" \"$CONTYNU_REHYDRATION_PACKET_FILE\" \"$CONTYNU_REHYDRATION_PROMPT_FILE\" > \"{}\"\nif [ -f AGENTS.md ]; then cp AGENTS.md \"{}\"; fi\nprintf mocked-codex\n",
            capture_path.display(),
            agents_capture_path.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&codex_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&codex_path, perms).unwrap();
    }
    let path = env::var("PATH").unwrap_or_default();
    let combined_path = format!("{}:{}", bin_dir.display(), path);

    let start = contynu_cmd()
        .env("PATH", &combined_path)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("start-project")
        .output()
        .unwrap();
    assert!(start.status.success());
    let project_id = extract_prefixed_id(&String::from_utf8_lossy(&start.stdout), "prj_");

    let codex = contynu_cmd()
        .env("PATH", &combined_path)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--cwd")
        .arg(dir.path())
        .arg("codex")
        .arg("--")
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        codex.status.success(),
        "launcher execution failed: {}",
        String::from_utf8_lossy(&codex.stderr)
    );
    let stdout = String::from_utf8_lossy(&codex.stdout);
    let stderr = String::from_utf8_lossy(&codex.stderr);
    assert!(stdout.contains("mocked-codex"));
    assert!(stderr.contains("Let's contynu another time."));
    let captured = fs::read_to_string(&capture_path).unwrap();
    assert!(captured.contains("env:"));
    assert!(captured.contains("rehydration.json"));
    assert!(captured.contains("prompt:"));
    assert!(captured.contains("rehydration.txt"));
    let agents = fs::read_to_string(&agents_capture_path).unwrap();
    assert!(agents.contains("<!-- contynu:codex:start -->"));
    assert!(agents.contains("# Contynu Working Continuation"));
    assert!(agents.contains("## Current Goal"));
    assert!(!dir.path().join("AGENTS.md").exists());

    // Verify project exists via status
    let status = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("status")
        .output()
        .unwrap();
    assert!(status.status.success());
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status_stdout.contains(&project_id));
}

#[test]
fn first_codex_launch_from_clean_state_reserves_project_and_registers_mcp() {
    let dir = tempdir().unwrap();
    let state_dir = env::current_dir()
        .unwrap()
        .join("target")
        .join("tmp")
        .join(format!(
            "contynu-smoke-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    let bin_dir = dir.path().join("bin");
    let home_dir = dir.path().join("home");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(home_dir.join(".codex")).unwrap();
    fs::create_dir_all(&state_dir).unwrap();
    fs::write(home_dir.join(".codex").join("config.toml"), "").unwrap();

    let codex_path = bin_dir.join("codex");
    let capture_path = dir.path().join("codex-first-launch.txt");
    fs::write(
        &codex_path,
        format!(
            "#!/bin/sh\nprintf \"project:%s|packet:%s\\n\" \"$CONTYNU_ACTIVE_PROJECT\" \"$CONTYNU_REHYDRATION_PACKET_FILE\" > \"{}\"\nprintf mocked-codex\n",
            capture_path.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&codex_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&codex_path, perms).unwrap();
    }

    let path = env::var("PATH").unwrap_or_default();
    let combined_path = format!("{}:{}", bin_dir.display(), path);

    let launch = contynu_cmd()
        .env("PATH", &combined_path)
        .env("HOME", &home_dir)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--cwd")
        .arg(dir.path())
        .arg("codex")
        .output()
        .unwrap();
    assert!(
        launch.status.success(),
        "clean-state codex launch failed: {}",
        String::from_utf8_lossy(&launch.stderr)
    );

    let captured = fs::read_to_string(&capture_path).unwrap();
    assert!(captured.contains("project:prj_"));
    assert!(captured.contains("rehydration.json"));

    let codex_config = fs::read_to_string(home_dir.join(".codex").join("config.toml")).unwrap();
    assert!(codex_config.contains("[mcp_servers.contynu]"));
    assert!(codex_config.contains("CONTYNU_ACTIVE_PROJECT = \"prj_"));

    let status = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("status")
        .output()
        .unwrap();
    assert!(status.status.success());
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status_stdout.contains("Project status"));
    assert!(status_stdout.contains("prj_"));

    let _ = fs::remove_dir_all(&state_dir);
}

#[test]
fn direct_passthrough_launches_regular_commands() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");

    let command = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--cwd")
        .arg(dir.path())
        .arg("bash")
        .arg("-lc")
        .arg("printf direct && printf direct > direct.txt")
        .output()
        .unwrap();
    assert!(
        command.status.success(),
        "direct passthrough failed: {}",
        String::from_utf8_lossy(&command.stderr)
    );
    let stdout = String::from_utf8_lossy(&command.stdout);
    let stderr = String::from_utf8_lossy(&command.stderr);
    assert!(stdout.contains("direct"));
    assert!(stderr.contains("Let's contynu another time."));
    assert_eq!(
        fs::read_to_string(dir.path().join("direct.txt")).unwrap(),
        "direct"
    );
}

#[test]
fn status_projects_and_config_commands_work() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");

    let run = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--cwd")
        .arg(dir.path())
        .arg("bash")
        .arg("-lc")
        .arg("printf status-smoke")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "initial run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let status = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("status")
        .output()
        .unwrap();
    assert!(status.status.success());
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status_stdout.contains("Project status"));
    assert!(status_stdout.contains("Active memories"));

    let projects = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("projects")
        .output()
        .unwrap();
    assert!(projects.status.success());
    let projects_stdout = String::from_utf8_lossy(&projects.stdout);
    assert!(projects_stdout.contains("Projects"));
    assert!(projects_stdout.contains("primary"));

    let config_validate = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("config")
        .arg("validate")
        .output()
        .unwrap();
    assert!(config_validate.status.success());
    let config_validate_stdout = String::from_utf8_lossy(&config_validate.stdout);
    assert!(config_validate_stdout.contains("Config is valid."));
    assert!(config_validate_stdout.contains("delivery:"));

    let config_show = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("config")
        .arg("show")
        .output()
        .unwrap();
    assert!(config_show.status.success());
    let config_show_stdout = String::from_utf8_lossy(&config_show.stdout);
    assert!(config_show_stdout.contains("Config file:"));
    assert!(config_show_stdout.contains("\"command\": \"codex\""));
}

#[test]
fn new_flag_prompts_and_wipes_history_before_starting_fresh() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");

    let initial = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--cwd")
        .arg(dir.path())
        .arg("bash")
        .arg("-lc")
        .arg("printf first")
        .output()
        .unwrap();
    assert!(initial.status.success());

    let mut fresh = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--new")
        .arg("--cwd")
        .arg(dir.path())
        .arg("bash")
        .arg("-lc")
        .arg("printf second")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    fresh.stdin.as_mut().unwrap().write_all(b"yes\n").unwrap();
    let output = fresh.wait_with_output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("permanently wipe the chat history"));
    assert!(stderr.contains("Type `yes` to continue"));

    // Verify we can still run status after wipe (new project was created)
    let status = contynu_cmd()
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("status")
        .output()
        .unwrap();
    assert!(status.status.success());
}

#[test]
fn builtin_launcher_config_can_override_known_launcher_behavior() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&state_dir).unwrap();
    fs::write(
        state_dir.join("config.json"),
        r#"{
          "llm_launchers": [
            {
              "command": "codex",
              "aliases": ["codex-cli"],
              "hydrate": true,
              "hydration_delivery": "env_only",
              "hydration_args": ["--context-file", "{prompt_file}"],
              "extra_env": {"CODEX_PROFILE": "custom"}
            }
          ]
        }"#,
    )
    .unwrap();

    let codex_path = bin_dir.join("codex");
    let capture_path = dir.path().join("codex-config-capture.txt");
    fs::write(
        &codex_path,
        format!(
            "#!/bin/sh\nprintf \"arg1:%s|arg2:%s|env:%s|profile:%s\\n\" \"$1\" \"$2\" \"$CONTYNU_REHYDRATION_PACKET_FILE\" \"$CODEX_PROFILE\" > \"{}\"\nprintf mocked-codex\n",
            capture_path.display(),
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&codex_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&codex_path, perms).unwrap();
    }
    let path = env::var("PATH").unwrap_or_default();
    let combined_path = format!("{}:{}", bin_dir.display(), path);

    let start = contynu_cmd()
        .env("PATH", &combined_path)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("start-project")
        .output()
        .unwrap();
    assert!(start.status.success());

    let launch = contynu_cmd()
        .env("PATH", &combined_path)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--cwd")
        .arg(dir.path())
        .arg("codex")
        .output()
        .unwrap();
    assert!(
        launch.status.success(),
        "configured codex launcher failed: {}",
        String::from_utf8_lossy(&launch.stderr)
    );
    let captured = fs::read_to_string(&capture_path).unwrap();
    assert!(captured.contains("arg1:--context-file"));
    assert!(captured.contains("arg2:"));
    assert!(captured.contains("rehydration.txt"));
    assert!(captured.contains("env:"));
    assert!(captured.contains("rehydration.json"));
    assert!(captured.contains("profile:custom"));
    assert!(!captured.contains("CONTYNU REHYDRATION CONTEXT"));
}

#[test]
fn configured_custom_llm_launcher_is_hydrated() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&state_dir).unwrap();
    fs::write(
        state_dir.join("config.json"),
        r#"{
          "llm_launchers": [
            {
              "command": "futurellm",
              "aliases": ["futurellm-cli"],
              "hydrate": true,
              "hydration_delivery": "env_only",
              "hydration_args": ["--context-file", "{prompt_file}", "--project", "{project_id}"],
              "extra_env": {"FUTURELLM_MODE": "enabled"}
            }
          ]
        }"#,
    )
    .unwrap();

    let future_path = bin_dir.join("futurellm");
    let capture_path = dir.path().join("futurellm-capture.txt");
    fs::write(
        &future_path,
        format!(
            "#!/bin/sh\nprintf \"arg1:%s|arg2:%s|env:%s|extra:%s\\n\" \"$1\" \"$2\" \"$CONTYNU_REHYDRATION_PACKET_FILE\" \"$FUTURELLM_MODE\" > \"{}\"\nprintf futurellm\n",
            capture_path.display(),
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&future_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&future_path, perms).unwrap();
    }
    let path = env::var("PATH").unwrap_or_default();
    let combined_path = format!("{}:{}", bin_dir.display(), path);

    let start = contynu_cmd()
        .env("PATH", &combined_path)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("start-project")
        .output()
        .unwrap();
    assert!(start.status.success());
    let project_id = extract_prefixed_id(&String::from_utf8_lossy(&start.stdout), "prj_");

    let launch = contynu_cmd()
        .env("PATH", &combined_path)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--cwd")
        .arg(dir.path())
        .arg("futurellm")
        .output()
        .unwrap();
    assert!(
        launch.status.success(),
        "configured launcher failed: {}",
        String::from_utf8_lossy(&launch.stderr)
    );
    let captured = fs::read_to_string(&capture_path).unwrap();
    assert!(captured.contains("arg1:--context-file"));
    assert!(captured.contains("arg2:"));
    assert!(captured.contains("rehydration.txt"));
    assert!(captured.contains("rehydration.json"));
    assert!(captured.contains("extra:enabled"));
    assert!(!captured.contains("CONTYNU REHYDRATION CONTEXT"));
    assert!(captured.contains(&project_id));
}

#[test]
fn startup_update_check_prints_exact_manual_command_for_current_environment() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");
    let release_base = spawn_release_server("v9.9.9", "#!/usr/bin/env sh\nexit 0\n".into());

    let mut command = Command::new(env!("CARGO_BIN_EXE_contynu"));
    let output = command
        .env_remove("CONTYNU_SKIP_UPDATE_CHECK")
        .env("CONTYNU_FORCE_INTERACTIVE_UPDATE_PROMPT", "1")
        .env("CONTYNU_RELEASE_API_URL", format!("{release_base}/latest"))
        .env(
            "CONTYNU_RELEASE_DOWNLOAD_BASE_URL",
            format!("{release_base}/downloads"),
        )
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("doctor")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut child = output;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"manual\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("A newer Contynu release is available"));
    assert!(stderr.contains("Run this command to update:"));
    assert!(stderr.contains(current_installer_asset()));
    assert!(
        stderr.contains("CONTYNU_INSTALL_DIR=") || stderr.contains("$env:CONTYNU_INSTALL_DIR=")
    );
    assert!(!state_dir.join("config.json").exists());
}

#[test]
fn startup_update_check_can_auto_update_for_current_environment() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");
    let install_dir = dir.path().join("install-bin");
    fs::create_dir_all(&install_dir).unwrap();
    let release_base = spawn_release_server(
        "v9.9.9",
        "#!/usr/bin/env sh\nset -eu\nmkdir -p \"$CONTYNU_INSTALL_DIR\"\nprintf updated > \"$CONTYNU_INSTALL_DIR/contynu\"\nchmod 0755 \"$CONTYNU_INSTALL_DIR/contynu\"\n".into(),
    );

    let mut command = Command::new(env!("CARGO_BIN_EXE_contynu"));
    let output = command
        .env_remove("CONTYNU_SKIP_UPDATE_CHECK")
        .env("CONTYNU_FORCE_INTERACTIVE_UPDATE_PROMPT", "1")
        .env("CONTYNU_RELEASE_API_URL", format!("{release_base}/latest"))
        .env(
            "CONTYNU_RELEASE_DOWNLOAD_BASE_URL",
            format!("{release_base}/downloads"),
        )
        .env("CONTYNU_INSTALL_DIR", &install_dir)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("doctor")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut child = output;
    child.stdin.as_mut().unwrap().write_all(b"auto\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("updated to v9.9.9"));
    assert_eq!(
        fs::read_to_string(install_dir.join("contynu")).unwrap(),
        "updated"
    );
    assert!(!state_dir.join("config.json").exists());
}
