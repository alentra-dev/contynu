use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::adapters::AdapterKind;
use crate::blobs::BlobStore;
use crate::checkpoint::CheckpointManager;
use crate::error::{ContynuError, Result};
use crate::event::{Actor, EventDraft, EventType};
use crate::files::{FileChangeKind, FileTracker};
use crate::ids::{ArtifactId, FileId, SessionId, TurnId};
use crate::journal::Journal;
use crate::state::StatePaths;
use crate::store::{ArtifactRecord, FileRecord, MetadataStore, SessionRecord, TurnRecord};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub state_dir: PathBuf,
    pub cwd: PathBuf,
    pub command: Vec<OsString>,
    pub ignore_patterns: Vec<String>,
    pub checkpoint_on_exit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub exit_code: Option<i32>,
    pub interrupted: bool,
}

pub struct RuntimeEngine;

impl RuntimeEngine {
    pub fn run(config: RunConfig) -> Result<RunOutcome> {
        if config.command.is_empty() {
            return Err(ContynuError::Validation(
                "no command provided after `--`".into(),
            ));
        }

        let state = StatePaths::new(&config.state_dir);
        state.ensure_layout()?;
        let store = MetadataStore::open(state.sqlite_db())?;
        let blob_store = BlobStore::new(state.blobs_root());
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let journal = Journal::open(state.journal_path_for_session(&session_id))?;
        let adapter = AdapterKind::detect(&config.command[0].to_string_lossy());
        let tracker = FileTracker::new(&config.cwd, &config.ignore_patterns)?;
        let before = tracker.snapshot()?;

        store.register_session(&SessionRecord {
            session_id: session_id.clone(),
            project_id: None,
            status: "started".into(),
            cli_name: Some(adapter.as_str().into()),
            cli_version: None,
            model_name: None,
            cwd: Some(config.cwd.display().to_string()),
            repo_root: Some(config.cwd.display().to_string()),
            host_fingerprint: None,
            started_at: Utc::now(),
            ended_at: None,
        })?;
        store.register_turn(&TurnRecord {
            turn_id: turn_id.clone(),
            session_id: session_id.clone(),
            status: "started".into(),
            started_at: Utc::now(),
            completed_at: None,
            summary_memory_id: None,
        })?;

        Self::persist(
            &journal,
            &store,
            EventDraft::new(
                session_id.clone(),
                None,
                Actor::Adapter,
                EventType::AdapterAttached,
                json!({
                    "adapter_kind": adapter.as_str(),
                    "program": config.command[0].to_string_lossy(),
                }),
            ),
        )?;
        Self::persist(
            &journal,
            &store,
            EventDraft::new(
                session_id.clone(),
                None,
                Actor::System,
                EventType::SessionStarted,
                json!({
                    "cwd": config.cwd.display().to_string(),
                    "adapter_kind": adapter.as_str(),
                }),
            ),
        )?;
        Self::persist(
            &journal,
            &store,
            EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::Runtime,
                EventType::TurnStarted,
                json!({"status": "started"}),
            ),
        )?;
        Self::persist(
            &journal,
            &store,
            EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::Runtime,
                EventType::ProcessStarted,
                json!({
                    "command": config.command.iter().map(|item| item.to_string_lossy().to_string()).collect::<Vec<_>>(),
                    "cwd": config.cwd.display().to_string(),
                }),
            ),
        )?;

        let mut command = Command::new(&config.command[0]);
        command.args(&config.command[1..]);
        command.current_dir(&config.cwd);
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let child = command
            .spawn()
            .map_err(|error| ContynuError::CommandStart(error.to_string()))?;

        let child = Arc::new(Mutex::new(child));
        let interrupted = Arc::new(AtomicBool::new(false));
        let child_for_signal = Arc::clone(&child);
        let interrupted_for_signal = Arc::clone(&interrupted);
        ctrlc::set_handler(move || {
            interrupted_for_signal.store(true, Ordering::SeqCst);
            if let Ok(mut child) = child_for_signal.lock() {
                let _ = child.kill();
            }
        })
        .ok();

        let (stdout, stderr, status) = {
            let mut child = child
                .lock()
                .map_err(|_| ContynuError::Validation("child process mutex poisoned".into()))?;
            let stdout = {
                let mut buffer = Vec::new();
                if let Some(stdout) = child.stdout.as_mut() {
                    use std::io::Read;
                    stdout.read_to_end(&mut buffer)?;
                }
                buffer
            };
            let stderr = {
                let mut buffer = Vec::new();
                if let Some(stderr) = child.stderr.as_mut() {
                    use std::io::Read;
                    stderr.read_to_end(&mut buffer)?;
                }
                buffer
            };
            let status = child.wait()?;
            (stdout, stderr, status)
        };

        if !stdout.is_empty() {
            let stdout_text = String::from_utf8_lossy(&stdout).into_owned();
            Self::persist(
                &journal,
                &store,
                EventDraft::new(
                    session_id.clone(),
                    Some(turn_id.clone()),
                    Actor::Runtime,
                    EventType::StdoutCaptured,
                    json!({"text": stdout_text}),
                ),
            )?;
        }
        if !stderr.is_empty() {
            let stderr_text = String::from_utf8_lossy(&stderr).into_owned();
            Self::persist(
                &journal,
                &store,
                EventDraft::new(
                    session_id.clone(),
                    Some(turn_id.clone()),
                    Actor::Runtime,
                    EventType::StderrCaptured,
                    json!({"text": stderr_text}),
                ),
            )?;
        }

        Self::persist(
            &journal,
            &store,
            EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::Runtime,
                EventType::ProcessExited,
                json!({
                    "exit_code": status.code(),
                    "success": status.success(),
                    "interrupted": interrupted.load(Ordering::SeqCst),
                }),
            ),
        )?;

        let after = tracker.snapshot()?;
        for change in tracker.diff(&before, &after) {
            let snapshot_event_type = match change.kind {
                FileChangeKind::Added | FileChangeKind::Modified => EventType::FileSnapshot,
                FileChangeKind::Deleted => EventType::FileDeleted,
            };
            let payload = json!({
                "path": change.path,
                "kind": match change.kind {
                    FileChangeKind::Added => "added",
                    FileChangeKind::Modified => "modified",
                    FileChangeKind::Deleted => "deleted",
                },
                "before_sha256": change.before_sha256,
                "after_sha256": change.after_sha256,
                "diff": change.diff,
            });
            let (event, append) = journal.append(EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::Filesystem,
                snapshot_event_type,
                payload,
            ))?;
            store.record_event(&event, &journal.path().display().to_string(), append)?;

            if let Some(snapshot) = change.snapshot {
                let bytes = std::fs::read(&snapshot.absolute_path)?;
                let blob = blob_store.put_bytes(&bytes)?;
                store.register_blob(&blob, None)?;
                store.register_file(&FileRecord {
                    file_id: FileId::new(),
                    session_id: session_id.clone(),
                    workspace_relative_path: snapshot.relative_path.clone(),
                    kind: if snapshot.is_text {
                        "text".into()
                    } else {
                        "binary".into()
                    },
                    last_known_sha256: Some(snapshot.sha256.clone()),
                    last_snapshot_event_id: Some(event.event_id.clone()),
                    last_diff_event_id: None,
                    observed_at: Utc::now(),
                    is_generated: !snapshot.relative_path.ends_with(".rs")
                        && !snapshot.relative_path.ends_with(".md")
                        && !snapshot.relative_path.ends_with(".toml"),
                })?;

                if !snapshot.is_text || snapshot.size_bytes > 128 * 1024 {
                    store.register_artifact(&ArtifactRecord {
                        artifact_id: ArtifactId::new(),
                        session_id: session_id.clone(),
                        source_event_id: event.event_id.clone(),
                        path: Some(snapshot.relative_path),
                        kind: "file_output".into(),
                        mime_type: None,
                        sha256: blob.sha256.clone(),
                        blob_relative_path: blob.relative_path,
                        size_bytes: blob.size_bytes,
                        created_at: Utc::now(),
                    })?;
                }
            }
        }

        Self::persist(
            &journal,
            &store,
            EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::Runtime,
                EventType::TurnCompleted,
                json!({"exit_code": status.code(), "interrupted": interrupted.load(Ordering::SeqCst)}),
            ),
        )?;
        Self::persist(
            &journal,
            &store,
            EventDraft::new(
                session_id.clone(),
                None,
                Actor::System,
                if interrupted.load(Ordering::SeqCst) {
                    EventType::SessionInterrupted
                } else {
                    EventType::SessionEnded
                },
                json!({"exit_code": status.code()}),
            ),
        )?;

        if config.checkpoint_on_exit {
            let manager = CheckpointManager::new(&state, &store, &blob_store);
            let _ = manager.create_checkpoint(&journal, &session_id, "run_completed", None)?;
        }

        Ok(RunOutcome {
            session_id,
            turn_id,
            exit_code: status.code(),
            interrupted: interrupted.load(Ordering::SeqCst),
        })
    }

    fn persist(journal: &Journal, store: &MetadataStore, draft: EventDraft) -> Result<()> {
        let (event, append) = journal.append(draft)?;
        store.record_event(&event, &journal.path().display().to_string(), append)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{RunConfig, RuntimeEngine};
    use crate::{MetadataStore, StatePaths};

    #[test]
    fn runtime_run_captures_process_and_files() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let state = dir.path().join(".contynu");

        let outcome = RuntimeEngine::run(RunConfig {
            state_dir: state.clone(),
            cwd: workspace.clone(),
            command: vec![
                "bash".into(),
                "-lc".into(),
                "printf hello && printf world >&2 && printf sample > output.txt".into(),
            ],
            ignore_patterns: Vec::new(),
            checkpoint_on_exit: true,
        })
        .unwrap();

        assert_eq!(outcome.exit_code, Some(0));
        let paths = StatePaths::new(state);
        let store = MetadataStore::open(paths.sqlite_db()).unwrap();
        let events = store.list_events_for_session(&outcome.session_id).unwrap();
        assert!(events
            .iter()
            .any(|event| event.event_type == "process_started"));
        assert!(events
            .iter()
            .any(|event| event.event_type == "stdout_captured"));
        assert!(events
            .iter()
            .any(|event| event.event_type == "stderr_captured"));
        assert!(workspace.join("output.txt").exists());
    }
}
