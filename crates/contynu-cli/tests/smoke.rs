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
