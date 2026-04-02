use std::process::Command;
use std::{env, fs};

use tempfile::tempdir;

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
    assert!(stdout.contains("state_root"));
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
    let project_id = String::from_utf8_lossy(&start.stdout).trim().to_string();
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
    assert!(run_stdout.contains(&project_id));

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
            "#!/bin/sh\nprintf \"env:%s\\n\" \"$CONTYNU_REHYDRATION_PACKET_FILE\" > \"{}\"\ncat >> \"{}\"\nprintf mocked-codex\n",
            capture_path.display(),
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
    let project_id = String::from_utf8_lossy(&start.stdout).trim().to_string();

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
    assert!(stdout.contains(&project_id));
    let captured = fs::read_to_string(&capture_path).unwrap();
    assert!(captured.contains("env:"));
    assert!(captured.contains("rehydration.json"));
    assert!(captured.contains("CONTYNU REHYDRATION CONTEXT"));
    assert!(captured.contains(&project_id));
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
        .arg("printf direct > direct.txt")
        .output()
        .unwrap();
    assert!(
        command.status.success(),
        "direct passthrough failed: {}",
        String::from_utf8_lossy(&command.stderr)
    );
    let stdout = String::from_utf8_lossy(&command.stdout);
    assert!(stdout.contains("\"project_id\""));
    assert_eq!(
        fs::read_to_string(dir.path().join("direct.txt")).unwrap(),
        "direct"
    );
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
    let project_id = String::from_utf8_lossy(&start.stdout).trim().to_string();

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
