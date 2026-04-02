use std::process::Command;

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
