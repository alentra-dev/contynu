use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};
use std::thread;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::adapters::{AdapterSpec, HydrationContext};
use crate::blobs::BlobStore;
use crate::checkpoint::CheckpointManager;
use crate::config::ContynuConfig;
use crate::error::{ContynuError, Result};
use crate::event::{Actor, EventDraft, EventType};
use crate::files::{FileChangeKind, FileTracker};
use crate::ids::{ArtifactId, FileId, ProjectId, SessionId, TurnId};
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
    pub project_id: Option<ProjectId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    pub project_id: ProjectId,
    pub turn_id: TurnId,
    pub exit_code: Option<i32>,
    pub interrupted: bool,
}

pub struct RuntimeEngine;

const STREAM_CHUNK_SIZE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Stdout,
    Stderr,
}

#[derive(Debug)]
enum StreamMessage {
    Chunk { kind: StreamKind, bytes: Vec<u8> },
    Closed { kind: StreamKind },
}

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
        let config_file = ContynuConfig::load(&state.config_path())?;
        let blob_store = BlobStore::new(state.blobs_root());
        let resolved_project = config.project_id.clone().or(store.primary_project_id()?);
        let continuing_session = resolved_project.is_some();
        let session_id = match resolved_project {
            Some(project_id) => {
                if !store.session_exists(&project_id)? {
                    return Err(ContynuError::Validation(format!(
                        "project `{project_id}` does not exist"
                    )));
                }
                project_id
            }
            None => ProjectId::new(),
        };
        let turn_id = TurnId::new();
        let journal = Journal::open(state.journal_path_for_session(&session_id))?;
        let adapter = AdapterSpec::detect(&config.command[0].to_string_lossy(), &config_file);
        let tracker = FileTracker::new(&config.cwd, &config.ignore_patterns)?;
        let before = tracker.snapshot()?;

        if continuing_session {
            store.update_session_status(&session_id, "active", None)?;
        } else {
            store.register_session(&SessionRecord {
                session_id: session_id.clone(),
                project_id: Some(session_id.to_string()),
                status: "active".into(),
                cli_name: Some(adapter.as_str().into()),
                cli_version: None,
                model_name: None,
                cwd: Some(config.cwd.display().to_string()),
                repo_root: Some(config.cwd.display().to_string()),
                host_fingerprint: None,
                started_at: Utc::now(),
                ended_at: None,
            })?;
            store.set_primary_project_id(&session_id)?;
        }
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
                    "adapter_type": format!("{:?}", adapter.kind()).to_lowercase(),
                    "program": config.command[0].to_string_lossy(),
                }),
            ),
        )?;

        let hydration = if adapter.should_hydrate() && continuing_session {
            Some(Self::prepare_hydration(
                &state,
                &store,
                &blob_store,
                &journal,
                &session_id,
                &turn_id,
                adapter.as_str(),
            )?)
        } else {
            None
        };
        Self::persist(
            &journal,
            &store,
            EventDraft::new(
                session_id.clone(),
                None,
                Actor::System,
                if continuing_session {
                    EventType::SessionResumed
                } else {
                    EventType::SessionStarted
                },
                json!({
                    "cwd": config.cwd.display().to_string(),
                    "adapter_kind": adapter.as_str(),
                    "continued": continuing_session,
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

        let launch_plan = adapter.build_launch_plan(
            config.command[0].clone(),
            config.command[1..].to_vec(),
            hydration.as_ref(),
        )?;

        let mut command = Command::new(&launch_plan.executable);
        command.args(&launch_plan.args);
        command.current_dir(&config.cwd);
        if launch_plan.stdin_prelude.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.envs(launch_plan.env.iter().map(|(key, value)| (key, value)));

        let mut child = command
            .spawn()
            .map_err(|error| ContynuError::CommandStart(error.to_string()))?;

        if let Some(stdin_prelude) = launch_plan.stdin_prelude {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(&stdin_prelude)?;
                stdin.flush()?;
            }
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ContynuError::InvalidState("missing stdout pipe".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ContynuError::InvalidState("missing stderr pipe".into()))?;

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

        let (sender, receiver) = mpsc::channel();
        let stdout_handle = spawn_reader(stdout, StreamKind::Stdout, sender.clone());
        let stderr_handle = spawn_reader(stderr, StreamKind::Stderr, sender);

        let (stdout_bytes, stderr_bytes) =
            Self::capture_streams(receiver, &journal, &store, &session_id, &turn_id)?;
        stdout_handle
            .join()
            .map_err(|_| ContynuError::InvalidState("stdout reader thread panicked".into()))??;
        stderr_handle
            .join()
            .map_err(|_| ContynuError::InvalidState("stderr reader thread panicked".into()))??;
        let status = child
            .lock()
            .map_err(|_| ContynuError::Validation("child process mutex poisoned".into()))?
            .wait()?;

        Self::register_stream_artifacts(
            &journal,
            &store,
            &blob_store,
            &session_id,
            &turn_id,
            &stdout_bytes,
            &stderr_bytes,
        )?;

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
        store.update_turn_status(&turn_id, "completed", Some(Utc::now()))?;
        store.update_session_status(
            &session_id,
            if interrupted.load(Ordering::SeqCst) {
                "interrupted"
            } else {
                "active"
            },
            None,
        )?;

        if config.checkpoint_on_exit {
            let manager = CheckpointManager::new(&state, &store, &blob_store);
            let _ = manager.create_checkpoint(&journal, &session_id, "run_completed", None)?;
        }

        Ok(RunOutcome {
            project_id: session_id,
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

    fn capture_streams(
        receiver: Receiver<StreamMessage>,
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut stdout_closed = false;
        let mut stderr_closed = false;
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();

        while !(stdout_closed && stderr_closed) {
            match receiver
                .recv()
                .map_err(|_| ContynuError::InvalidState("stream capture channel closed".into()))?
            {
                StreamMessage::Chunk { kind, bytes } => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    let event_type = match kind {
                        StreamKind::Stdout => EventType::StdoutCaptured,
                        StreamKind::Stderr => EventType::StderrCaptured,
                    };
                    if kind == StreamKind::Stdout {
                        stdout_bytes.extend_from_slice(&bytes);
                    } else {
                        stderr_bytes.extend_from_slice(&bytes);
                    }
                    Self::persist(
                        journal,
                        store,
                        EventDraft::new(
                            session_id.clone(),
                            Some(turn_id.clone()),
                            Actor::Runtime,
                            event_type,
                            json!({
                                "text": text,
                                "stream": match kind {
                                    StreamKind::Stdout => "stdout",
                                    StreamKind::Stderr => "stderr",
                                },
                                "bytes": bytes.len(),
                            }),
                        ),
                    )?;
                }
                StreamMessage::Closed { kind } => match kind {
                    StreamKind::Stdout => stdout_closed = true,
                    StreamKind::Stderr => stderr_closed = true,
                },
            }
        }

        Ok((stdout_bytes, stderr_bytes))
    }

    fn register_stream_artifacts(
        journal: &Journal,
        store: &MetadataStore,
        blob_store: &BlobStore,
        session_id: &SessionId,
        turn_id: &TurnId,
        stdout_bytes: &[u8],
        stderr_bytes: &[u8],
    ) -> Result<()> {
        for (kind, bytes, mime_type) in [
            ("stdout_capture", stdout_bytes, "text/plain"),
            ("stderr_capture", stderr_bytes, "text/plain"),
        ] {
            if bytes.is_empty() {
                continue;
            }

            let blob = blob_store.put_bytes(bytes)?;
            store.register_blob(&blob, Some(mime_type))?;
            let (event, append) = journal.append(EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::Runtime,
                EventType::ArtifactRegistered,
                json!({
                    "artifact_kind": kind,
                    "sha256": blob.sha256,
                    "size_bytes": blob.size_bytes,
                }),
            ))?;
            store.record_event(&event, &journal.path().display().to_string(), append)?;
            store.register_artifact(&ArtifactRecord {
                artifact_id: ArtifactId::new(),
                session_id: session_id.clone(),
                source_event_id: event.event_id.clone(),
                path: None,
                kind: kind.into(),
                mime_type: Some(mime_type.into()),
                sha256: blob.sha256.clone(),
                blob_relative_path: blob.relative_path,
                size_bytes: blob.size_bytes,
                created_at: Utc::now(),
            })?;
        }

        Ok(())
    }

    fn prepare_hydration(
        state: &StatePaths,
        store: &MetadataStore,
        blob_store: &BlobStore,
        journal: &Journal,
        project_id: &ProjectId,
        turn_id: &TurnId,
        adapter_name: &str,
    ) -> Result<HydrationContext> {
        let manager = CheckpointManager::new(state, store, blob_store);
        let packet = manager.build_packet(project_id, None)?;
        let runtime_dir = state.project_runtime_dir(project_id);
        std::fs::create_dir_all(&runtime_dir)?;
        let packet_path = runtime_dir.join("rehydration.json");
        let prompt_path = runtime_dir.join("rehydration.txt");
        let packet_json = serde_json::to_string_pretty(&packet)?;
        let prompt = format!(
            "Project continuity context for {}.\nUse the JSON packet as authoritative state.\n{}",
            adapter_name, packet_json
        );
        std::fs::write(&packet_path, &packet_json)?;
        std::fs::write(&prompt_path, &prompt)?;

        let packet_blob = blob_store.put_text(&packet_json)?;
        store.register_blob(&packet_blob, Some("application/json"))?;
        let prompt_blob = blob_store.put_text(&prompt)?;
        store.register_blob(&prompt_blob, Some("text/plain"))?;
        let (event, append) = journal.append(EventDraft::new(
            project_id.clone(),
            Some(turn_id.clone()),
            Actor::System,
            EventType::RehydrationPacketCreated,
            json!({
                "adapter_kind": adapter_name,
                "packet_sha256": packet_blob.sha256,
                "prompt_sha256": prompt_blob.sha256,
                "packet_path": packet_path.display().to_string(),
                "prompt_path": prompt_path.display().to_string(),
            }),
        ))?;
        store.record_event(&event, &journal.path().display().to_string(), append)?;

        Ok(HydrationContext {
            project_id: project_id.clone(),
            packet,
            packet_path,
            prompt_path,
        })
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    kind: StreamKind,
    sender: Sender<StreamMessage>,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || loop {
        let mut buffer = vec![0_u8; STREAM_CHUNK_SIZE];
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            let _ = sender.send(StreamMessage::Closed { kind });
            return Ok(());
        }
        buffer.truncate(read);
        sender
            .send(StreamMessage::Chunk {
                kind,
                bytes: buffer,
            })
            .map_err(|_| ContynuError::InvalidState("stream capture receiver dropped".into()))?;
    })
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::{RunConfig, RuntimeEngine};
    use crate::{Journal, MetadataStore, StatePaths};

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
            project_id: None,
        })
        .unwrap();

        assert_eq!(outcome.exit_code, Some(0));
        let paths = StatePaths::new(state);
        let store = MetadataStore::open(paths.sqlite_db()).unwrap();
        let events = store.list_events_for_session(&outcome.project_id).unwrap();
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

    #[test]
    fn runtime_persists_stream_output_before_process_exit() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let state = dir.path().join(".contynu");
        let state_for_thread = state.clone();
        let workspace_for_thread = workspace.clone();

        let handle = thread::spawn(move || {
            RuntimeEngine::run(RunConfig {
                state_dir: state_for_thread,
                cwd: workspace_for_thread,
                command: vec![
                    "bash".into(),
                    "-lc".into(),
                    "printf early && sleep 1 && printf done".into(),
                ],
                ignore_patterns: Vec::new(),
                checkpoint_on_exit: false,
                project_id: None,
            })
            .unwrap()
        });

        thread::sleep(Duration::from_millis(300));

        let journal_root = StatePaths::new(&state).journal_root();
        let mut entries = std::fs::read_dir(&journal_root)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.path());
        assert_eq!(entries.len(), 1);

        let journal = Journal::open(entries[0].path()).unwrap();
        let replay = journal.replay().unwrap();
        assert!(replay
            .iter()
            .any(|item| item.event.event_type.as_str() == "stdout_captured"));

        let outcome = handle.join().unwrap();
        assert_eq!(outcome.exit_code, Some(0));
    }

    #[test]
    fn runtime_can_continue_existing_session_with_new_turn() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let state = dir.path().join(".contynu");

        let first = RuntimeEngine::run(RunConfig {
            state_dir: state.clone(),
            cwd: workspace.clone(),
            command: vec!["bash".into(), "-lc".into(), "printf first".into()],
            ignore_patterns: Vec::new(),
            checkpoint_on_exit: false,
            project_id: None,
        })
        .unwrap();

        let second = RuntimeEngine::run(RunConfig {
            state_dir: state.clone(),
            cwd: workspace,
            command: vec!["bash".into(), "-lc".into(), "printf second".into()],
            ignore_patterns: Vec::new(),
            checkpoint_on_exit: false,
            project_id: Some(first.project_id.clone()),
        })
        .unwrap();

        assert_eq!(first.project_id, second.project_id);
        assert_ne!(first.turn_id, second.turn_id);

        let store = MetadataStore::open(StatePaths::new(state).sqlite_db()).unwrap();
        let events = store.list_events_for_session(&first.project_id).unwrap();
        assert!(events
            .iter()
            .any(|event| event.event_type == "session_resumed"));
    }
}
