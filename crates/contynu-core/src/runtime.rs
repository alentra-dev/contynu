use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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
use crate::files::{FileChange, FileChangeKind, FileRole, FileTracker};
use crate::ids::{ArtifactId, FileId, MemoryId, ProjectId, SessionId, TurnId};
use crate::journal::Journal;
use crate::state::StatePaths;
use crate::store::{
    ArtifactRecord, FileRecord, MemoryObject, MemoryObjectKind, MetadataStore, SessionRecord,
    TurnRecord,
};

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
    Pty,
}

impl StreamKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Pty => "pty",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ExecutionTransport {
    Pipes,
    Pty,
}

impl ExecutionTransport {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pipes => "pipes",
            Self::Pty => "pty",
        }
    }
}

#[derive(Debug)]
struct ProcessCapture {
    status: std::process::ExitStatus,
    stdout_bytes: Vec<u8>,
    stderr_bytes: Vec<u8>,
}

struct WorkspaceContextGuard {
    path: PathBuf,
    original: Option<Vec<u8>>,
}

impl Drop for WorkspaceContextGuard {
    fn drop(&mut self) {
        if let Some(original) = &self.original {
            let _ = std::fs::write(&self.path, original);
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
    }
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
        let transport = resolve_transport(&adapter);
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
                    "transport": transport.as_str(),
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
                    "transport": transport.as_str(),
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
                    "transport": transport.as_str(),
                }),
            ),
        )?;

        let launch_plan = adapter.build_launch_plan(
            config.command[0].clone(),
            config.command[1..].to_vec(),
            hydration.as_ref(),
        )?;
        let _context_guard = if let (Some(hydration), Some(context_file)) =
            (hydration.as_ref(), adapter.context_file())
        {
            Some(Self::install_workspace_context(
                &config.cwd,
                context_file,
                hydration,
                &journal,
                &store,
                &session_id,
                &turn_id,
            )?)
        } else {
            None
        };

        let interrupted = Arc::new(AtomicBool::new(false));
        let capture = Self::execute_launch_plan(
            &config.cwd,
            &launch_plan,
            transport,
            &journal,
            &store,
            &session_id,
            &turn_id,
            Arc::clone(&interrupted),
        )?;

        Self::register_stream_artifacts(
            &journal,
            &store,
            &blob_store,
            &session_id,
            &turn_id,
            transport,
            &capture.stdout_bytes,
            &capture.stderr_bytes,
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
                    "exit_code": capture.status.code(),
                    "success": capture.status.success(),
                    "interrupted": interrupted.load(Ordering::SeqCst),
                    "transport": transport.as_str(),
                }),
            ),
        )?;

        let after = tracker.snapshot()?;
        let changes = tracker.diff(&before, &after);
        let mut change_event_ids = Vec::new();
        for change in &changes {
            let snapshot_event_type = match change.kind {
                FileChangeKind::Added | FileChangeKind::Modified => EventType::FileSnapshot,
                FileChangeKind::Deleted => EventType::FileDeleted,
            };
            let payload = json!({
                "path": change.path,
                "role": change.role.as_str(),
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
            change_event_ids.push(event.event_id.clone());

            let mut diff_event_id = None;
            if let Some(diff) = &change.diff {
                let (diff_event, diff_append) = journal.append(EventDraft::new(
                    session_id.clone(),
                    Some(turn_id.clone()),
                    Actor::Filesystem,
                    EventType::FileDiff,
                    json!({
                        "path": change.path,
                        "role": change.role.as_str(),
                        "diff": diff,
                    }),
                ))?;
                store.record_event(
                    &diff_event,
                    &journal.path().display().to_string(),
                    diff_append,
                )?;
                change_event_ids.push(diff_event.event_id.clone());
                diff_event_id = Some(diff_event.event_id.clone());
            }

            if let Some(snapshot) = &change.snapshot {
                let bytes = std::fs::read(&snapshot.absolute_path)?;
                let blob = blob_store.put_bytes(&bytes)?;
                store.register_blob(&blob, None)?;
                store.register_file(&FileRecord {
                    file_id: FileId::new(),
                    session_id: session_id.clone(),
                    workspace_relative_path: snapshot.relative_path.clone(),
                    kind: format!(
                        "{}_{}",
                        snapshot.role.as_str(),
                        if snapshot.is_text { "text" } else { "binary" }
                    ),
                    last_known_sha256: Some(snapshot.sha256.clone()),
                    last_snapshot_event_id: Some(event.event_id.clone()),
                    last_diff_event_id: diff_event_id,
                    observed_at: Utc::now(),
                    is_generated: snapshot.role != FileRole::Source,
                })?;

                if snapshot.role != FileRole::Source
                    || !snapshot.is_text
                    || snapshot.size_bytes > 128 * 1024
                {
                    store.register_artifact(&ArtifactRecord {
                        artifact_id: ArtifactId::new(),
                        session_id: session_id.clone(),
                        source_event_id: event.event_id.clone(),
                        path: Some(snapshot.relative_path.clone()),
                        kind: match snapshot.role {
                            FileRole::Source => "source_snapshot",
                            FileRole::Generated => "generated_file_output",
                            FileRole::Artifact => "artifact_output",
                        }
                        .into(),
                        mime_type: None,
                        sha256: blob.sha256.clone(),
                        blob_relative_path: blob.relative_path,
                        size_bytes: blob.size_bytes,
                        created_at: Utc::now(),
                    })?;
                }
            } else if change.kind == FileChangeKind::Deleted {
                store.register_file(&FileRecord {
                    file_id: FileId::new(),
                    session_id: session_id.clone(),
                    workspace_relative_path: change.path.clone(),
                    kind: format!("{}_deleted", change.role.as_str()),
                    last_known_sha256: None,
                    last_snapshot_event_id: Some(event.event_id.clone()),
                    last_diff_event_id: diff_event_id,
                    observed_at: Utc::now(),
                    is_generated: change.role != FileRole::Source,
                })?;
            }
        }
        Self::persist_workspace_scan_summary(
            &journal,
            &store,
            &session_id,
            &turn_id,
            &changes,
            transport,
        )?;
        Self::derive_memory_objects(
            &journal,
            &store,
            &session_id,
            &turn_id,
            &config.command,
            adapter.as_str(),
            transport,
            capture.status.code(),
            interrupted.load(Ordering::SeqCst),
            &changes,
            &change_event_ids,
        )?;

        Self::persist(
            &journal,
            &store,
            EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::Runtime,
                EventType::TurnCompleted,
                json!({"exit_code": capture.status.code(), "interrupted": interrupted.load(Ordering::SeqCst), "transport": transport.as_str()}),
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
                json!({"exit_code": capture.status.code(), "transport": transport.as_str()}),
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
            exit_code: capture.status.code(),
            interrupted: interrupted.load(Ordering::SeqCst),
        })
    }

    fn persist(journal: &Journal, store: &MetadataStore, draft: EventDraft) -> Result<()> {
        let (event, append) = journal.append(draft)?;
        store.record_event(&event, &journal.path().display().to_string(), append)?;
        Ok(())
    }

    fn execute_launch_plan(
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
        transport: ExecutionTransport,
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
        interrupted: Arc<AtomicBool>,
    ) -> Result<ProcessCapture> {
        match transport {
            ExecutionTransport::Pipes => Self::execute_with_pipes(
                cwd,
                launch_plan,
                journal,
                store,
                session_id,
                turn_id,
                interrupted,
            ),
            ExecutionTransport::Pty => Self::execute_with_pty(
                cwd,
                launch_plan,
                journal,
                store,
                session_id,
                turn_id,
                interrupted,
            ),
        }
    }

    fn execute_with_pipes(
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
        interrupted: Arc<AtomicBool>,
    ) -> Result<ProcessCapture> {
        let mut command = Command::new(&launch_plan.executable);
        command.args(&launch_plan.args);
        command.current_dir(cwd);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.envs(launch_plan.env.iter().map(|(key, value)| (key, value)));

        let mut child = command
            .spawn()
            .map_err(|error| ContynuError::CommandStart(error.to_string()))?;

        if let Some(mut stdin) = child.stdin.take() {
            if let Some(stdin_prelude) = &launch_plan.stdin_prelude {
                stdin.write_all(stdin_prelude)?;
                stdin.flush()?;
            }
            thread::spawn(move || {
                let mut input = std::io::stdin();
                let _ = std::io::copy(&mut input, &mut stdin);
                let _ = stdin.flush();
            });
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
        install_ctrlc_handler(Arc::clone(&child), interrupted);

        let (sender, receiver) = mpsc::channel();
        let stdout_handle = spawn_reader(stdout, StreamKind::Stdout, sender.clone());
        let stderr_handle = spawn_reader(stderr, StreamKind::Stderr, sender);

        let (stdout_bytes, stderr_bytes) =
            Self::capture_streams(receiver, journal, store, session_id, turn_id)?;
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

        Ok(ProcessCapture {
            status,
            stdout_bytes,
            stderr_bytes,
        })
    }

    fn execute_with_pty(
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
        interrupted: Arc<AtomicBool>,
    ) -> Result<ProcessCapture> {
        let mut script_command = Command::new("script");
        let command_text = std::iter::once(&launch_plan.executable)
            .chain(launch_plan.args.iter())
            .map(shell_escape)
            .collect::<Vec<_>>()
            .join(" ");
        script_command.args(["-qefc", &command_text, "/dev/null"]);
        script_command.current_dir(cwd);
        script_command.stdin(Stdio::piped());
        script_command.stdout(Stdio::piped());
        script_command.stderr(Stdio::piped());
        script_command.envs(launch_plan.env.iter().map(|(key, value)| (key, value)));

        let mut child = script_command
            .spawn()
            .map_err(|error| ContynuError::CommandStart(error.to_string()))?;

        if let Some(mut stdin) = child.stdin.take() {
            if let Some(stdin_prelude) = &launch_plan.stdin_prelude {
                stdin.write_all(stdin_prelude)?;
                stdin.flush()?;
            }
            thread::spawn(move || {
                let mut input = std::io::stdin();
                let _ = std::io::copy(&mut input, &mut stdin);
                let _ = stdin.flush();
            });
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ContynuError::InvalidState("missing pty stdout pipe".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ContynuError::InvalidState("missing pty stderr pipe".into()))?;

        let child = Arc::new(Mutex::new(child));
        install_ctrlc_handler(Arc::clone(&child), interrupted);

        let (sender, receiver) = mpsc::channel();
        let stdout_handle = spawn_reader(stdout, StreamKind::Pty, sender.clone());
        let stderr_handle = spawn_reader(stderr, StreamKind::Stderr, sender);

        let (stdout_bytes, stderr_bytes) =
            Self::capture_streams(receiver, journal, store, session_id, turn_id)?;
        stdout_handle
            .join()
            .map_err(|_| ContynuError::InvalidState("pty reader thread panicked".into()))??;
        stderr_handle.join().map_err(|_| {
            ContynuError::InvalidState("pty stderr reader thread panicked".into())
        })??;
        let status = child
            .lock()
            .map_err(|_| ContynuError::Validation("child process mutex poisoned".into()))?
            .wait()?;

        Ok(ProcessCapture {
            status,
            stdout_bytes,
            stderr_bytes,
        })
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
                        StreamKind::Pty => EventType::StdoutCaptured,
                    };
                    if kind == StreamKind::Stdout {
                        stdout_bytes.extend_from_slice(&bytes);
                    } else if kind == StreamKind::Stderr {
                        stderr_bytes.extend_from_slice(&bytes);
                    } else {
                        stdout_bytes.extend_from_slice(&bytes);
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
                                "stream": kind.as_str(),
                                "bytes": bytes.len(),
                            }),
                        ),
                    )?;
                }
                StreamMessage::Closed { kind } => match kind {
                    StreamKind::Stdout | StreamKind::Pty => stdout_closed = true,
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
        transport: ExecutionTransport,
        stdout_bytes: &[u8],
        stderr_bytes: &[u8],
    ) -> Result<()> {
        let captures = if matches!(transport, ExecutionTransport::Pty) {
            vec![("pty_capture", stdout_bytes, "text/plain")]
        } else {
            vec![
                ("stdout_capture", stdout_bytes, "text/plain"),
                ("stderr_capture", stderr_bytes, "text/plain"),
            ]
        };
        for (kind, bytes, mime_type) in captures {
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

    fn persist_workspace_scan_summary(
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
        changes: &[FileChange],
        transport: ExecutionTransport,
    ) -> Result<()> {
        let source = changes
            .iter()
            .filter(|change| change.role == FileRole::Source)
            .count();
        let generated = changes
            .iter()
            .filter(|change| change.role == FileRole::Generated)
            .count();
        let artifacts = changes
            .iter()
            .filter(|change| change.role == FileRole::Artifact)
            .count();
        Self::persist(
            journal,
            store,
            EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::Filesystem,
                EventType::WorkspaceScanCompleted,
                json!({
                    "transport": transport.as_str(),
                    "changed_files": changes.len(),
                    "source_changes": source,
                    "generated_changes": generated,
                    "artifact_changes": artifacts,
                }),
            ),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn derive_memory_objects(
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
        command: &[OsString],
        adapter_name: &str,
        transport: ExecutionTransport,
        exit_code: Option<i32>,
        interrupted: bool,
        changes: &[FileChange],
        change_event_ids: &[crate::ids::EventId],
    ) -> Result<()> {
        let mut source_event_ids = store
            .list_events_for_turn(session_id, turn_id)?
            .into_iter()
            .map(|event| event.event_id)
            .collect::<Vec<_>>();
        source_event_ids.extend(change_event_ids.iter().cloned());
        source_event_ids.sort();
        source_event_ids.dedup();
        let command_text = command
            .iter()
            .map(|item| item.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        let summary = format!(
            "Last turn used `{}` via `{}` over `{}` and exited with {:?}. Changed {} files.",
            command_text,
            adapter_name,
            transport.as_str(),
            exit_code,
            changes.len()
        );
        Self::insert_memory_object(
            journal,
            store,
            session_id,
            turn_id,
            MemoryObjectKind::Summary,
            summary,
            Some(0.9),
            true,
            source_event_ids.clone(),
        )?;
        Self::insert_memory_object(
            journal,
            store,
            session_id,
            turn_id,
            MemoryObjectKind::Fact,
            format!(
                "Command `{}` exited with {:?} using {} transport.",
                command_text,
                exit_code,
                transport.as_str()
            ),
            Some(0.8),
            false,
            source_event_ids.clone(),
        )?;
        if interrupted {
            Self::insert_memory_object(
                journal,
                store,
                session_id,
                turn_id,
                MemoryObjectKind::Todo,
                "The previous turn was interrupted and may need manual continuation.".into(),
                Some(0.8),
                false,
                source_event_ids.clone(),
            )?;
        } else if exit_code.unwrap_or_default() != 0 {
            Self::insert_memory_object(
                journal,
                store,
                session_id,
                turn_id,
                MemoryObjectKind::Todo,
                format!(
                    "Investigate non-zero exit from `{}`: {:?}.",
                    command_text, exit_code
                ),
                Some(0.85),
                false,
                source_event_ids.clone(),
            )?;
        }
        for change in changes {
            let note = match change.role {
                FileRole::Source => format!("Changed source file {}.", change.path),
                FileRole::Generated => format!("Produced generated file {}.", change.path),
                FileRole::Artifact => format!("Produced artifact file {}.", change.path),
            };
            Self::insert_memory_object(
                journal,
                store,
                session_id,
                turn_id,
                MemoryObjectKind::FileNote,
                note,
                Some(0.7),
                false,
                source_event_ids.clone(),
            )?;
        }
        Ok(())
    }

    fn insert_memory_object(
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
        kind: MemoryObjectKind,
        text: String,
        confidence: Option<f64>,
        supersede_kind: bool,
        source_event_ids: Vec<crate::ids::EventId>,
    ) -> Result<Option<MemoryId>> {
        if store
            .find_active_memory_by_text(session_id, kind, &text)?
            .is_some()
        {
            return Ok(None);
        }
        let memory_id = MemoryId::new();
        if supersede_kind {
            store.supersede_memory_kind(session_id, kind, &memory_id)?;
        }
        let memory = MemoryObject {
            memory_id: memory_id.clone(),
            session_id: session_id.clone(),
            kind,
            status: "active".into(),
            text: text.clone(),
            confidence,
            source_event_ids,
            created_at: Utc::now(),
            superseded_by: None,
        };
        store.insert_memory_object(&memory)?;
        if kind == MemoryObjectKind::Summary {
            store.set_turn_summary_memory(turn_id, &memory_id)?;
        }
        Self::persist(
            journal,
            store,
            EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::System,
                EventType::MemoryObjectDerived,
                json!({
                    "memory_id": memory_id,
                    "kind": kind.as_str(),
                    "text": text,
                    "confidence": confidence,
                }),
            ),
        )?;
        Ok(Some(memory_id))
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
            prompt_text: prompt,
        })
    }

    fn install_workspace_context(
        cwd: &Path,
        context_file: &str,
        hydration: &HydrationContext,
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
    ) -> Result<WorkspaceContextGuard> {
        let path = cwd.join(context_file);
        let original = std::fs::read(&path).ok();
        let merged = merge_workspace_context(original.as_deref(), &hydration.prompt_text);
        std::fs::write(&path, merged.as_bytes())?;
        Self::persist(
            journal,
            store,
            EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::System,
                EventType::ArtifactMaterialized,
                json!({
                    "kind": "workspace_context_file",
                    "path": path.display().to_string(),
                    "created": original.is_none(),
                }),
            ),
        )?;
        Ok(WorkspaceContextGuard { path, original })
    }
}

fn resolve_transport(adapter: &AdapterSpec) -> ExecutionTransport {
    if adapter.use_pty() && cfg!(unix) && script_available() {
        ExecutionTransport::Pty
    } else {
        ExecutionTransport::Pipes
    }
}

fn script_available() -> bool {
    Command::new("script")
        .arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn shell_escape(value: &OsString) -> String {
    let value = value.to_string_lossy();
    if value.is_empty() {
        return "''".into();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':'))
    {
        return value.into_owned();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn merge_workspace_context(original: Option<&[u8]>, prompt_text: &str) -> String {
    const BEGIN: &str = "\n<!-- CONTYNU_BEGIN -->\n";
    const END: &str = "\n<!-- CONTYNU_END -->\n";

    let base = original
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default();
    let cleaned = if let (Some(start), Some(end)) = (base.find(BEGIN), base.find(END)) {
        let end = end + END.len();
        let mut merged = String::new();
        merged.push_str(&base[..start]);
        merged.push_str(&base[end..]);
        merged
    } else {
        base
    };
    let mut merged = cleaned.trim_end().to_string();
    if !merged.is_empty() {
        merged.push_str("\n\n");
    }
    merged
        .push_str("Contynu continuity context. Use this as authoritative current project state.\n");
    merged.push_str(BEGIN.trim_start_matches('\n'));
    merged.push_str(prompt_text);
    merged.push_str(END);
    merged
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

fn install_ctrlc_handler(child: Arc<Mutex<std::process::Child>>, interrupted: Arc<AtomicBool>) {
    ctrlc::set_handler(move || {
        interrupted.store(true, Ordering::SeqCst);
        if let Ok(mut child) = child.lock() {
            let _ = child.kill();
        }
    })
    .ok();
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::{RunConfig, RuntimeEngine};
    use crate::{Journal, MemoryObjectKind, MetadataStore, StatePaths};

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
        assert!(events
            .iter()
            .any(|event| event.event_type == "workspace_scan_completed"));
        assert!(workspace.join("output.txt").exists());

        let memory = store
            .list_memory_objects(&outcome.project_id, None)
            .unwrap();
        assert!(memory
            .iter()
            .any(|item| item.kind == MemoryObjectKind::Summary));
        assert!(memory
            .iter()
            .any(|item| item.kind == MemoryObjectKind::FileNote));
        assert!(memory.iter().all(|item| !item.source_event_ids.is_empty()));

        let turn_events = store
            .list_events_for_turn(&outcome.project_id, &outcome.turn_id)
            .unwrap();
        assert!(!turn_events.is_empty());
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

    #[test]
    fn runtime_can_use_pty_transport_for_configured_launcher() {
        if !super::script_available() {
            return;
        }

        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let state = dir.path().join(".contynu");
        std::fs::create_dir_all(&state).unwrap();

        let launcher = dir.path().join("mock-pty-llm");
        let tty_path = workspace.join("tty.txt");
        std::fs::write(
            state.join("config.json"),
            format!(
                r#"{{
                  "llm_launchers": [
                    {{
                      "command": "{}",
                      "hydrate": true,
                      "use_pty": true,
                      "hydration_delivery": "env_only"
                    }}
                  ]
                }}"#,
                launcher.display()
            ),
        )
        .unwrap();
        std::fs::write(
            &launcher,
            format!(
                "#!/bin/sh\ntty > \"{}\"\nprintf pty-ok\n",
                tty_path.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&launcher).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&launcher, perms).unwrap();
        }

        let outcome = RuntimeEngine::run(RunConfig {
            state_dir: state.clone(),
            cwd: workspace.clone(),
            command: vec![launcher.into_os_string()],
            ignore_patterns: Vec::new(),
            checkpoint_on_exit: false,
            project_id: None,
        })
        .unwrap();

        assert_eq!(outcome.exit_code, Some(0));
        let tty_output = std::fs::read_to_string(tty_path).unwrap();
        assert!(tty_output.contains("/dev/"));

        let store = MetadataStore::open(StatePaths::new(state).sqlite_db()).unwrap();
        let events = store.list_events_for_session(&outcome.project_id).unwrap();
        assert!(events.iter().any(|event| {
            event.event_type == "stdout_captured"
                && event
                    .payload_json
                    .to_string()
                    .contains("\"stream\":\"pty\"")
        }));
    }
}
