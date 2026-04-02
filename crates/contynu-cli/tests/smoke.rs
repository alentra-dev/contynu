use std::process::Command;
use std::{env, fs};

use tempfile::tempdir;

fn extract_prefixed_id(output: &str, prefix: &str) -> String {
    output
        .split_whitespace()
        .find(|token| token.starts_with(prefix))
        .unwrap_or_else(|| panic!("missing {prefix} in output: {output}"))
        .to_string()
}

#[test]
fn init_and_doctor_work() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");

    let init = Command::new(env!("CARGO_BIN_EXE_contynu"))
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

    let doctor = Command::new(env!("CARGO_BIN_EXE_contynu"))
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
fn project_is_created_and_reused_by_default() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");

    let start = Command::new(env!("CARGO_BIN_EXE_contynu"))
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

    let run = Command::new(env!("CARGO_BIN_EXE_contynu"))
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
    assert!(run_stderr.contains("Saved turn trn_"));

    let inspect = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("inspect")
        .arg("project")
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "inspect project failed: {}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let inspect_stdout = String::from_utf8_lossy(&inspect.stdout);
    assert!(inspect_stdout.contains(&project_id));
    assert!(inspect_stdout.contains("session_resumed"));
}

#[test]
fn streamlined_launcher_reuses_primary_project() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let codex_path = bin_dir.join("codex");
    let capture_path = dir.path().join("codex-capture.txt");
    fs::write(
        &codex_path,
        format!(
            "#!/bin/sh\nprintf \"env:%s|ctx:%s\\n\" \"$CONTYNU_REHYDRATION_PACKET_FILE\" \"$(grep -c CONTYNU_BEGIN AGENTS.md 2>/dev/null || true)\" > \"{}\"\ngrep -q CONTYNU_BEGIN AGENTS.md && grep -q CONTYNU_END AGENTS.md && grep -q authoritative AGENTS.md\nprintf mocked-codex\n",
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

    let start = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("start-project")
        .output()
        .unwrap();
    assert!(start.status.success());
    let project_id = extract_prefixed_id(&String::from_utf8_lossy(&start.stdout), "prj_");

    let codex = Command::new(env!("CARGO_BIN_EXE_contynu"))
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
    assert!(captured.contains("ctx:1"));
    assert!(!dir.path().join("AGENTS.md").exists());

    let inspect = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("inspect")
        .arg("project")
        .output()
        .unwrap();
    assert!(inspect.status.success());
    let inspect_stdout = String::from_utf8_lossy(&inspect.stdout);
    assert!(inspect_stdout.contains(&project_id));
}

#[test]
fn direct_passthrough_launches_regular_commands() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");

    let command = Command::new(env!("CARGO_BIN_EXE_contynu"))
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
fn status_projects_recent_and_config_commands_work() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".contynu");

    let run = Command::new(env!("CARGO_BIN_EXE_contynu"))
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

    let status = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("status")
        .output()
        .unwrap();
    assert!(status.status.success());
    let status_stdout = String::from_utf8_lossy(&status.stdout);
    assert!(status_stdout.contains("Project status"));
    assert!(status_stdout.contains("Counts"));

    let projects = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("projects")
        .output()
        .unwrap();
    assert!(projects.status.success());
    let projects_stdout = String::from_utf8_lossy(&projects.stdout);
    assert!(projects_stdout.contains("Projects"));
    assert!(projects_stdout.contains("primary"));

    let recent = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("recent")
        .output()
        .unwrap();
    assert!(recent.status.success());
    let recent_stdout = String::from_utf8_lossy(&recent.stdout);
    assert!(recent_stdout.contains("Recent activity"));
    assert!(recent_stdout.contains("latest turn:"));

    let config_validate = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("config")
        .arg("validate")
        .output()
        .unwrap();
    assert!(config_validate.status.success());
    let config_validate_stdout = String::from_utf8_lossy(&config_validate.stdout);
    assert!(config_validate_stdout.contains("Config is valid."));
    assert!(config_validate_stdout.contains("context file:"));

    let config_show = Command::new(env!("CARGO_BIN_EXE_contynu"))
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

    let initial = Command::new(env!("CARGO_BIN_EXE_contynu"))
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

    let mut fresh = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--new")
        .arg("--cwd")
        .arg(dir.path())
        .arg("bash")
        .arg("-lc")
        .arg("printf second")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    use std::io::Write as _;
    fresh.stdin.as_mut().unwrap().write_all(b"yes\n").unwrap();
    let output = fresh.wait_with_output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("permanently wipe the chat history"));
    assert!(stderr.contains("Type `yes` to continue"));

    let inspect = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("inspect")
        .arg("project")
        .output()
        .unwrap();
    assert!(inspect.status.success());
    let inspect_stdout = String::from_utf8_lossy(&inspect.stdout);
    assert!(!inspect_stdout.contains("first"));
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

    let start = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .env("PATH", &combined_path)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("start-project")
        .output()
        .unwrap();
    assert!(start.status.success());

    let launch = Command::new(env!("CARGO_BIN_EXE_contynu"))
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

    let start = Command::new(env!("CARGO_BIN_EXE_contynu"))
        .env("PATH", &combined_path)
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("start-project")
        .output()
        .unwrap();
    assert!(start.status.success());
    let project_id = extract_prefixed_id(&String::from_utf8_lossy(&start.stdout), "prj_");

    let launch = Command::new(env!("CARGO_BIN_EXE_contynu"))
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
