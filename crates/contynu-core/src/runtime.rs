use std::ffi::OsString;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
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
use crate::ids::{ArtifactId, MemoryId, ProjectId, SessionId, TurnId};
use crate::journal::Journal;
use crate::pty::PtyChild;
use crate::state::StatePaths;
use crate::store::{
    ArtifactRecord, EventRecord, MemoryObject, MemoryObjectKind, MetadataStore, SessionRecord,
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
    InheritTerminal,
}

impl ExecutionTransport {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pipes => "pipes",
            Self::Pty => "pty",
            Self::InheritTerminal => "inherit_terminal",
        }
    }
}

#[derive(Debug)]
struct ProcessCapture {
    stdin_bytes: Vec<u8>,
    exit_code: Option<i32>,
    success: bool,
    stdout_bytes: Vec<u8>,
    stderr_bytes: Vec<u8>,
}

struct MemoryCandidate {
    kind: MemoryObjectKind,
    text: String,
    confidence: f64,
}

struct StartupIndicator {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl StartupIndicator {
    fn start(message: &'static str) -> Self {
        if !std::io::stderr().is_terminal() {
            return Self {
                stop: Arc::new(AtomicBool::new(true)),
                handle: None,
            };
        }

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut index = 0_usize;
            while !thread_stop.load(Ordering::SeqCst) {
                eprint!("\r\x1b[2K{} {}", frames[index % frames.len()], message);
                let _ = std::io::stderr().flush();
                index += 1;
                thread::sleep(std::time::Duration::from_millis(90));
            }
            eprint!("\r\x1b[2K");
            let _ = std::io::stderr().flush();
        });

        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Debug)]
enum StreamMessage {
    Chunk { kind: StreamKind, bytes: Vec<u8> },
    Closed { kind: StreamKind },
}

#[derive(Default)]
struct PendingTranscript {
    stdin: Option<PathBuf>,
    stdout: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct DialogueCapture {
    prompts: Vec<String>,
    responses: Vec<String>,
}

enum TranscriptStream {
    Stdin,
    Stdout,
}

impl RuntimeEngine {
    pub fn run(config: RunConfig) -> Result<RunOutcome> {
        if config.command.is_empty() {
            return Err(ContynuError::Validation(
                "no command provided after `--`".into(),
            ));
        }

        let mut startup_indicator =
            StartupIndicator::start("Contynu is restoring continuity for this run...");
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
        Self::reconcile_pending_transcripts(&state, &journal, &store, &session_id)?;
        let adapter = AdapterSpec::detect(&config.command[0].to_string_lossy(), &config_file);
        let transport = resolve_transport(&adapter);
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
                &adapter,
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

        // Write context files for adapters that read project instructions from files.
        let context_file = if let Some(ref hydration) = hydration {
            write_context_file(&config.cwd, &adapter, &hydration.prompt_text)?
        } else {
            None
        };

        startup_indicator.stop();

        let interrupted = Arc::new(AtomicBool::new(false));
        let capture = Self::execute_launch_plan(
            &state,
            &config.cwd,
            &launch_plan,
            transport,
            &journal,
            &store,
            &session_id,
            &turn_id,
            Arc::clone(&interrupted),
        )?;

        // Restore original context file after execution.
        if let Some(ref path) = context_file {
            cleanup_context_file(path);
        }

        Self::register_stream_artifacts(
            &journal,
            &store,
            &blob_store,
            &session_id,
            &turn_id,
            transport,
            &capture.stdin_bytes,
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
                    "exit_code": capture.exit_code,
                    "success": capture.success,
                    "interrupted": interrupted.load(Ordering::SeqCst),
                    "transport": transport.as_str(),
                }),
            ),
        )?;

        Self::derive_memory_objects(
            &journal,
            &store,
            &session_id,
            &turn_id,
            &config.command,
            adapter.as_str(),
            transport,
            capture.exit_code,
            interrupted.load(Ordering::SeqCst),
        )?;

        Self::persist(
            &journal,
            &store,
            EventDraft::new(
                session_id.clone(),
                Some(turn_id.clone()),
                Actor::Runtime,
                EventType::TurnCompleted,
                json!({"exit_code": capture.exit_code, "interrupted": interrupted.load(Ordering::SeqCst), "transport": transport.as_str()}),
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
                json!({"exit_code": capture.exit_code, "transport": transport.as_str()}),
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
            exit_code: capture.exit_code,
            interrupted: interrupted.load(Ordering::SeqCst),
        })
    }

    fn persist(journal: &Journal, store: &MetadataStore, draft: EventDraft) -> Result<()> {
        let (event, append) = journal.append(draft)?;
        store.record_event(&event, &journal.path().display().to_string(), append)?;
        Ok(())
    }

    fn reconcile_pending_transcripts(
        state: &StatePaths,
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
    ) -> Result<()> {
        let runtime_dir = state.project_runtime_dir(session_id);
        if !runtime_dir.exists() {
            return Ok(());
        }

        let mut pending = std::collections::BTreeMap::<TurnId, PendingTranscript>::new();
        for entry in std::fs::read_dir(&runtime_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if let Some((turn_id, stream)) = parse_transcript_log_name(session_id, name) {
                let transcript = pending.entry(turn_id).or_default();
                match stream {
                    TranscriptStream::Stdin => transcript.stdin = Some(path),
                    TranscriptStream::Stdout => transcript.stdout = Some(path),
                }
            }
        }

        for (turn_id, transcript) in pending {
            let events = store.list_events_for_turn(session_id, &turn_id)?;
            let has_stdin = events
                .iter()
                .any(|event| event.event_type == "stdin_captured");
            let has_stdout = events
                .iter()
                .any(|event| event.event_type == "stdout_captured");

            let stdin_bytes = transcript
                .stdin
                .as_ref()
                .map(|path| std::fs::read(path).unwrap_or_default())
                .unwrap_or_default();
            let stdout_bytes = transcript
                .stdout
                .as_ref()
                .map(|path| std::fs::read(path).unwrap_or_default())
                .unwrap_or_default();

            if !has_stdin || !has_stdout {
                let dialogue = extract_interactive_dialogue(&stdin_bytes, &stdout_bytes);
                if !has_stdin {
                    for prompt in &dialogue.prompts {
                        Self::persist(
                            journal,
                            store,
                            EventDraft::new(
                                session_id.clone(),
                                Some(turn_id.clone()),
                                Actor::Runtime,
                                EventType::StdinCaptured,
                                json!({
                                    "text": prompt,
                                    "stream": "stdin",
                                    "bytes": prompt.len(),
                                    "recovered": true,
                                }),
                            ),
                        )?;
                    }
                }
                if !has_stdout {
                    for response in &dialogue.responses {
                        Self::persist(
                            journal,
                            store,
                            EventDraft::new(
                                session_id.clone(),
                                Some(turn_id.clone()),
                                Actor::Runtime,
                                EventType::StdoutCaptured,
                                json!({
                                    "text": response,
                                    "stream": "stdout",
                                    "bytes": response.len(),
                                    "recovered": true,
                                }),
                            ),
                        )?;
                    }
                }
            }

            if let Some(path) = transcript.stdin.as_ref() {
                let _ = std::fs::remove_file(path);
            }
            if let Some(path) = transcript.stdout.as_ref() {
                let _ = std::fs::remove_file(path);
            }
        }

        Ok(())
    }

    fn execute_launch_plan(
        state: &StatePaths,
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
            ExecutionTransport::InheritTerminal => {
                #[cfg(unix)]
                {
                    Self::execute_with_script_logging(
                        state,
                        cwd,
                        launch_plan,
                        journal,
                        store,
                        session_id,
                        turn_id,
                        interrupted,
                    )
                }
                #[cfg(not(unix))]
                {
                    Self::execute_with_inherited_terminal(
                        state,
                        cwd,
                        launch_plan,
                        journal,
                        store,
                        session_id,
                        turn_id,
                        interrupted,
                    )
                }
            }
            ExecutionTransport::Pty => {
                #[cfg(unix)]
                {
                    Self::execute_with_pty(
                        cwd,
                        launch_plan,
                        journal,
                        store,
                        session_id,
                        turn_id,
                        interrupted,
                    )
                }
                #[cfg(not(unix))]
                {
                    Self::execute_with_pipes(
                        cwd,
                        launch_plan,
                        journal,
                        store,
                        session_id,
                        turn_id,
                        interrupted,
                    )
                }
            }
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
            stdin_bytes: Vec::new(),
            exit_code: status.code(),
            success: status.success(),
            stdout_bytes,
            stderr_bytes,
        })
    }

    #[allow(dead_code)]
    fn execute_with_inherited_terminal(
        _state: &StatePaths,
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
        _journal: &Journal,
        _store: &MetadataStore,
        _session_id: &SessionId,
        _turn_id: &TurnId,
        interrupted: Arc<AtomicBool>,
    ) -> Result<ProcessCapture> {
        let mut command = Command::new(&launch_plan.executable);
        command.args(&launch_plan.args);
        command.current_dir(cwd);
        command.stdin(Stdio::inherit());
        command.stdout(Stdio::inherit());
        command.stderr(Stdio::inherit());
        command.envs(launch_plan.env.iter().map(|(key, value)| (key, value)));

        let child = command
            .spawn()
            .map_err(|error| ContynuError::CommandStart(error.to_string()))?;

        let child = Arc::new(Mutex::new(child));
        install_ctrlc_handler(Arc::clone(&child), interrupted);
        let status = child
            .lock()
            .map_err(|_| ContynuError::Validation("child process mutex poisoned".into()))?
            .wait()?;

        Ok(ProcessCapture {
            stdin_bytes: Vec::new(),
            exit_code: status.code(),
            success: status.success(),
            stdout_bytes: Vec::new(),
            stderr_bytes: Vec::new(),
        })
    }

    #[cfg(unix)]
    fn execute_with_pty(
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
        interrupted: Arc<AtomicBool>,
    ) -> Result<ProcessCapture> {
        let child = PtyChild::spawn(
            cwd,
            &launch_plan.executable,
            &launch_plan.args,
            &launch_plan.env,
        )?;
        let mut stdin = child.try_clone_writer()?;
        let stdin_capture = Arc::new(Mutex::new(Vec::<u8>::new()));
        let (prelude_sender, prelude_receiver) = mpsc::channel::<()>();
        let stdin_prelude = launch_plan.stdin_prelude.clone();
        let stdin_capture_thread = Arc::clone(&stdin_capture);
        let stdin_is_tty = std::io::stdin().is_terminal();
        let stdin_handle = thread::spawn(move || {
            if let Some(stdin_prelude) = stdin_prelude.as_ref() {
                if stdin_is_tty {
                    // Interactive: wait for launcher readiness before injecting prelude.
                    match prelude_receiver.recv_timeout(std::time::Duration::from_secs(8)) {
                        Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                // Non-interactive (piped): send prelude immediately.
                if stdin.write_all(stdin_prelude).is_ok() {
                    let _ = stdin.flush();
                    if let Ok(mut captured) = stdin_capture_thread.lock() {
                        captured.extend_from_slice(stdin_prelude);
                    }
                }
            }
            let mut input = std::io::stdin();
            let mut buffer = vec![0_u8; STREAM_CHUNK_SIZE];
            loop {
                match input.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if stdin.write_all(&buffer[..read]).is_err() {
                            break;
                        }
                        if let Ok(mut captured) = stdin_capture_thread.lock() {
                            captured.extend_from_slice(&buffer[..read]);
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = stdin.flush();
        });

        let mut stdout = child.try_clone_reader()?;
        let stdout_capture = Arc::new(Mutex::new(Vec::<u8>::new()));
        install_pty_ctrlc_handler(&child, interrupted);
        let stdout_capture_thread = Arc::clone(&stdout_capture);
        let readiness_window = Arc::new(Mutex::new(Vec::<u8>::new()));
        let readiness_window_thread = Arc::clone(&readiness_window);
        let needs_prelude = launch_plan.stdin_prelude.is_some();
        let stdout_handle = thread::spawn(move || -> Result<()> {
            let mut prelude_sent = !needs_prelude;
            loop {
                let mut buffer = vec![0_u8; STREAM_CHUNK_SIZE];
                let read = match stdout.read(&mut buffer) {
                    Ok(read) => read,
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => return Ok(()),
                    Err(error) => return Err(error.into()),
                };
                if read == 0 {
                    return Ok(());
                }
                buffer.truncate(read);
                if let Ok(mut captured) = stdout_capture_thread.lock() {
                    captured.extend_from_slice(&buffer);
                }
                if !prelude_sent {
                    if let Ok(mut window) = readiness_window_thread.lock() {
                        window.extend_from_slice(&buffer);
                        if window.len() > 8192 {
                            let drop_len = window.len() - 8192;
                            window.drain(..drop_len);
                        }
                        if launcher_ready_for_prelude(window.as_slice()) {
                            let _ = prelude_sender.send(());
                            prelude_sent = true;
                        }
                    }
                }
                mirror_chunk_to_terminal(StreamKind::Pty, &buffer)?;
            }
        });
        let status = child.wait()?;
        stdin_handle
            .join()
            .map_err(|_| ContynuError::InvalidState("pty stdin thread panicked".into()))?;
        stdout_handle
            .join()
            .map_err(|_| ContynuError::InvalidState("pty reader thread panicked".into()))??;

        let stdin_bytes = stdin_capture
            .lock()
            .map_err(|_| ContynuError::Validation("pty stdin capture mutex poisoned".into()))?
            .clone();
        let stdout_bytes_raw = stdout_capture
            .lock()
            .map_err(|_| ContynuError::Validation("pty stdout capture mutex poisoned".into()))?
            .clone();
        let dialogue = extract_interactive_dialogue(&stdin_bytes, &stdout_bytes_raw);
        let stdout_text = dialogue.responses.join("\n\n");
        let stdout_bytes = stdout_text.as_bytes().to_vec();

        for prompt in &dialogue.prompts {
            Self::persist(
                journal,
                store,
                EventDraft::new(
                    session_id.clone(),
                    Some(turn_id.clone()),
                    Actor::Runtime,
                    EventType::StdinCaptured,
                    json!({
                        "text": prompt,
                        "stream": "stdin",
                        "bytes": prompt.len(),
                    }),
                ),
            )?;
        }
        for response in &dialogue.responses {
            Self::persist(
                journal,
                store,
                EventDraft::new(
                    session_id.clone(),
                    Some(turn_id.clone()),
                    Actor::Runtime,
                    EventType::StdoutCaptured,
                    json!({
                        "text": response,
                        "stream": "stdout",
                        "bytes": response.len(),
                    }),
                ),
            )?;
        }

        Ok(ProcessCapture {
            stdin_bytes,
            exit_code: status.code(),
            success: status.success(),
            stdout_bytes,
            stderr_bytes: Vec::new(),
        })
    }

    #[cfg(unix)]
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
    fn execute_with_script_logging(
        state: &StatePaths,
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
        interrupted: Arc<AtomicBool>,
    ) -> Result<ProcessCapture> {
        let runtime_dir = state.project_runtime_dir(session_id);
        std::fs::create_dir_all(&runtime_dir)?;
        let stdin_log = runtime_dir.join(format!(
            "{}--{}--stdin.log",
            session_id.as_str(),
            turn_id.as_str()
        ));
        let stdout_log = runtime_dir.join(format!(
            "{}--{}--stdout.log",
            session_id.as_str(),
            turn_id.as_str()
        ));

        let command_text = shell_command_text(&launch_plan.executable, &launch_plan.args);
        let mut command = Command::new("script");
        command.current_dir(cwd);
        command.stdin(Stdio::inherit());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.envs(launch_plan.env.iter().map(|(key, value)| (key, value)));
        command.arg("-qef");
        command.arg("--log-in");
        command.arg(&stdin_log);
        command.arg("--log-out");
        command.arg(&stdout_log);
        command.arg("--command");
        command.arg(command_text);

        let mut child = command
            .spawn()
            .map_err(|error| ContynuError::CommandStart(error.to_string()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ContynuError::InvalidState("missing script stdout pipe".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ContynuError::InvalidState("missing script stderr pipe".into()))?;

        let child = Arc::new(Mutex::new(child));
        install_ctrlc_handler(Arc::clone(&child), interrupted);
        let (sender, receiver) = mpsc::channel();
        let stdout_handle = spawn_reader(stdout, StreamKind::Stdout, sender.clone());
        let stderr_handle = spawn_reader(stderr, StreamKind::Stderr, sender);
        let mut handoff_indicator =
            StartupIndicator::start("Contynu is handing control to your launcher...");
        Self::drain_script_streams(receiver, &mut handoff_indicator)?;
        stdout_handle.join().map_err(|_| {
            ContynuError::InvalidState("script stdout reader thread panicked".into())
        })??;
        stderr_handle.join().map_err(|_| {
            ContynuError::InvalidState("script stderr reader thread panicked".into())
        })??;
        handoff_indicator.stop();
        let status = child
            .lock()
            .map_err(|_| ContynuError::Validation("child process mutex poisoned".into()))?
            .wait()?;

        let stdin_log_bytes = std::fs::read(&stdin_log).unwrap_or_default();
        let stdout_log_bytes = std::fs::read(&stdout_log).unwrap_or_default();
        let dialogue = extract_interactive_dialogue(&stdin_log_bytes, &stdout_log_bytes);
        let stdin_text = dialogue.prompts.join("\n\n");
        let stdout_text = dialogue.responses.join("\n\n");
        let stdin_bytes = stdin_text.as_bytes().to_vec();
        let stdout_bytes = stdout_text.as_bytes().to_vec();

        for prompt in &dialogue.prompts {
            Self::persist(
                journal,
                store,
                EventDraft::new(
                    session_id.clone(),
                    Some(turn_id.clone()),
                    Actor::Runtime,
                    EventType::StdinCaptured,
                    json!({
                        "text": prompt,
                        "stream": "stdin",
                        "bytes": prompt.len(),
                    }),
                ),
            )?;
        }
        for response in &dialogue.responses {
            Self::persist(
                journal,
                store,
                EventDraft::new(
                    session_id.clone(),
                    Some(turn_id.clone()),
                    Actor::Runtime,
                    EventType::StdoutCaptured,
                    json!({
                        "text": response,
                        "stream": "stdout",
                        "bytes": response.len(),
                    }),
                ),
            )?;
        }
        let _ = std::fs::remove_file(&stdin_log);
        let _ = std::fs::remove_file(&stdout_log);

        Ok(ProcessCapture {
            stdin_bytes,
            exit_code: status.code(),
            success: status.success(),
            stdout_bytes,
            stderr_bytes: Vec::new(),
        })
    }

    #[allow(dead_code)]
    fn drain_script_streams(
        receiver: Receiver<StreamMessage>,
        indicator: &mut StartupIndicator,
    ) -> Result<()> {
        let mut stdout_closed = false;
        let mut stderr_closed = false;
        let mut saw_output = false;

        while !(stdout_closed && stderr_closed) {
            match receiver
                .recv()
                .map_err(|_| ContynuError::InvalidState("script capture channel closed".into()))?
            {
                StreamMessage::Chunk { kind, bytes } => {
                    if !saw_output {
                        indicator.stop();
                        saw_output = true;
                    }
                    mirror_chunk_to_terminal(kind, &bytes)?;
                }
                StreamMessage::Closed { kind } => match kind {
                    StreamKind::Stdout | StreamKind::Pty => stdout_closed = true,
                    StreamKind::Stderr => stderr_closed = true,
                },
            }
        }

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
                        StreamKind::Pty => EventType::StdoutCaptured,
                    };
                    if kind == StreamKind::Stdout {
                        stdout_bytes.extend_from_slice(&bytes);
                    } else if kind == StreamKind::Stderr {
                        stderr_bytes.extend_from_slice(&bytes);
                    } else {
                        stdout_bytes.extend_from_slice(&bytes);
                    }
                    mirror_chunk_to_terminal(kind, &bytes)?;
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
        _stdin_bytes: &[u8],
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
    ) -> Result<()> {
        let source_events = store.list_events_for_turn(session_id, turn_id)?;
        let source_event_ids = source_events
            .iter()
            .map(|event| event.event_id.clone())
            .collect::<Vec<_>>();
        let command_text = command
            .iter()
            .map(|item| item.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        let latest_dialogue = latest_dialogue_from_events(&source_events);

        let source_adapter = Some(adapter_name.to_string());
        let source_model = store
            .get_session(session_id)
            .ok()
            .flatten()
            .and_then(|session| session.model_name);

        let summary = if let Some((prompt, response)) = latest_dialogue.as_ref() {
            format!(
                "Most recent exchange: user asked \"{}\" and the assistant answered \"{}\".",
                prompt, response
            )
        } else {
            format!(
                "Last turn used `{}` via `{}` over `{}` and exited with {:?}.",
                command_text,
                adapter_name,
                transport.as_str(),
                exit_code,
            )
        };
        Self::insert_memory_object(
            journal,
            store,
            session_id,
            turn_id,
            MemoryObjectKind::Summary,
            summary,
            Some(0.9),
            0.4, // summaries are replaceable
            true,
            source_event_ids.clone(),
            source_adapter.clone(),
            source_model.clone(),
        )?;
        if latest_dialogue.is_none() || interrupted || exit_code.unwrap_or_default() != 0 {
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
                0.7,
                false,
                source_event_ids.clone(),
                source_adapter.clone(),
                source_model.clone(),
            )?;
        }
        if interrupted {
            Self::insert_memory_object(
                journal,
                store,
                session_id,
                turn_id,
                MemoryObjectKind::Todo,
                "The previous turn was interrupted and may need manual continuation.".into(),
                Some(0.8),
                0.75,
                false,
                source_event_ids.clone(),
                source_adapter.clone(),
                source_model.clone(),
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
                0.75,
                false,
                source_event_ids.clone(),
                source_adapter.clone(),
                source_model.clone(),
            )?;
        }
        for candidate in derive_structured_candidates(&source_events, exit_code) {
            let importance = match candidate.kind {
                MemoryObjectKind::Constraint => 0.9,
                MemoryObjectKind::Decision => 0.85,
                MemoryObjectKind::Todo => 0.75,
                MemoryObjectKind::Fact => 0.7,
                _ => 0.6,
            };
            Self::insert_memory_object(
                journal,
                store,
                session_id,
                turn_id,
                candidate.kind,
                candidate.text,
                Some(candidate.confidence),
                importance,
                false,
                source_event_ids.clone(),
                source_adapter.clone(),
                source_model.clone(),
            )?;
        }

        // When there's no dialogue pairing (e.g. non-interactive -p mode), extract
        // the actual LLM response from stdout_captured as a Fact so it persists
        // across handoffs.
        if latest_dialogue.is_none() && exit_code == Some(0) {
            let stdout_text = extract_captured_response(&source_events);
            if let Some(response) = stdout_text {
                let truncated = if response.len() > 500 {
                    format!("{}...", &response[..500])
                } else {
                    response
                };
                Self::insert_memory_object(
                    journal,
                    store,
                    session_id,
                    turn_id,
                    MemoryObjectKind::Fact,
                    format!("LLM response: {}", truncated),
                    Some(0.85),
                    0.8,
                    false,
                    source_event_ids.clone(),
                    source_adapter.clone(),
                    source_model.clone(),
                )?;
            }
        }

        // Trigger consolidation if active memory count is high
        let active_count = store.count_active_memories(session_id, None)?;
        if active_count > 50 {
            Self::consolidate_memories(journal, store, session_id, turn_id)?;
        }

        Ok(())
    }

    fn consolidate_memories(
        journal: &Journal,
        store: &MetadataStore,
        session_id: &SessionId,
        turn_id: &TurnId,
    ) -> Result<()> {
        let consolidation_kinds = [
            MemoryObjectKind::Fact,
            MemoryObjectKind::Todo,
            MemoryObjectKind::Summary,
        ];
        let mut total_consolidated = 0usize;

        for kind in consolidation_kinds {
            let active = store.list_active_memory_objects(session_id, Some(kind))?;
            if active.len() <= 20 {
                continue;
            }

            // Sort by importance ASC -- take the bottom 50% for consolidation
            let mut sorted = active;
            sorted.sort_by(|a, b| a.importance.partial_cmp(&b.importance).unwrap_or(std::cmp::Ordering::Equal));
            let consolidation_count = sorted.len() / 2;
            let candidates = &sorted[..consolidation_count];

            // Group by simple word overlap (3+ shared content words)
            let mut groups: Vec<Vec<usize>> = Vec::new();
            let mut assigned = vec![false; candidates.len()];

            for i in 0..candidates.len() {
                if assigned[i] { continue; }
                let words_i: std::collections::HashSet<_> = candidates[i].text
                    .to_ascii_lowercase()
                    .split_whitespace()
                    .filter(|w| w.len() > 3)
                    .map(String::from)
                    .collect();
                let mut group = vec![i];
                assigned[i] = true;

                for j in (i + 1)..candidates.len() {
                    if assigned[j] { continue; }
                    let words_j: std::collections::HashSet<_> = candidates[j].text
                        .to_ascii_lowercase()
                        .split_whitespace()
                        .filter(|w| w.len() > 3)
                        .map(String::from)
                        .collect();
                    let shared = words_i.intersection(&words_j).count();
                    if shared >= 3 {
                        group.push(j);
                        assigned[j] = true;
                    }
                }

                if group.len() >= 2 {
                    groups.push(group);
                }
            }

            for group in &groups {
                let members: Vec<&MemoryObject> = group.iter().map(|&i| &candidates[i]).collect();
                let max_importance = members.iter().map(|m| m.importance).fold(0.0_f64, f64::max);
                let combined_text = members.iter()
                    .map(|m| m.text.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                let consolidated_from: Vec<MemoryId> = members.iter()
                    .map(|m| m.memory_id.clone())
                    .collect();

                let memory_id = MemoryId::new();
                // Supersede the originals
                for member in &members {
                    store.supersede_memory_kind_single(session_id, &member.memory_id, &memory_id)?;
                }

                let text_hash = Some(crate::event::sha256_hex(
                    combined_text.trim().to_lowercase().as_bytes(),
                ));
                let memory = MemoryObject {
                    memory_id: memory_id.clone(),
                    session_id: session_id.clone(),
                    kind,
                    status: "active".into(),
                    text: format!("[Consolidated] {}", combined_text),
                    confidence: Some(0.85),
                    source_event_ids: Vec::new(),
                    created_at: Utc::now(),
                    superseded_by: None,
                    source_adapter: Some("contynu_consolidation".into()),
                    source_model: None,
                    importance: max_importance,
                    access_count: 0,
                    last_accessed_at: None,
                    consolidated_from,
                    text_hash,
                };
                store.insert_memory_object(&memory)?;
                total_consolidated += group.len();
            }
        }

        if total_consolidated > 0 {
            Self::persist(
                journal,
                store,
                EventDraft::new(
                    session_id.clone(),
                    Some(turn_id.clone()),
                    Actor::System,
                    EventType::MemoryConsolidated,
                    json!({
                        "memories_consolidated": total_consolidated,
                    }),
                ),
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
        importance: f64,
        supersede_kind: bool,
        source_event_ids: Vec<crate::ids::EventId>,
        source_adapter: Option<String>,
        source_model: Option<String>,
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
        let text_hash = Some(crate::event::sha256_hex(
            text.trim().to_lowercase().as_bytes(),
        ));
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
            source_adapter: source_adapter.clone(),
            source_model: source_model.clone(),
            importance,
            access_count: 0,
            last_accessed_at: None,
            consolidated_from: Vec::new(),
            text_hash,
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
                    "importance": importance,
                    "source_adapter": source_adapter,
                    "source_model": source_model,
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
        adapter: &AdapterSpec,
    ) -> Result<HydrationContext> {
        let manager = CheckpointManager::new(state, store, blob_store);
        let packet = manager.build_packet(project_id, None)?;
        let runtime_dir = state.project_runtime_dir(project_id);
        std::fs::create_dir_all(&runtime_dir)?;
        let packet_path = runtime_dir.join("rehydration.json");
        let prompt_path = runtime_dir.join("rehydration.txt");
        let packet_json = serde_json::to_string_pretty(&packet)?;
        let format = adapter.prompt_format();
        let adapter_name = adapter.as_str();
        let prompt = crate::rendering::render_rehydration(&packet, format, adapter_name);
        let launcher_prompt = crate::rendering::render_launcher(&packet, format);
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
            launcher_prompt_text: launcher_prompt,
        })
    }
}

/// Write a context file (AGENTS.md, GEMINI.md) in the working directory so that
/// LLM CLIs pick up the rehydration context automatically. Returns the path if
/// a file was written, so it can be cleaned up after the session.
fn write_context_file(
    cwd: &std::path::Path,
    adapter: &AdapterSpec,
    prompt_text: &str,
) -> crate::error::Result<Option<std::path::PathBuf>> {
    let filename = match adapter.kind() {
        crate::AdapterKind::CodexCli => "AGENTS.md",
        crate::AdapterKind::GeminiCli => "GEMINI.md",
        // Claude uses --append-system-prompt; no file needed.
        // ConfiguredLlm and Terminal: no standard file.
        _ => return Ok(None),
    };

    let path = cwd.join(filename);

    // If the file already exists, prepend our context with a clear separator
    // and save the original so we can restore it.
    let backup_path = cwd.join(format!(".{}.contynu-backup", filename));
    if path.exists() {
        std::fs::copy(&path, &backup_path)?;
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(
            &path,
            format!(
                "{}\n\n---\n# Original {} content below\n---\n\n{}",
                prompt_text, filename, existing
            ),
        )?;
    } else {
        std::fs::write(&path, prompt_text)?;
    }

    Ok(Some(path))
}

/// Restore the original context file after session ends.
fn cleanup_context_file(path: &std::path::Path) {
    let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
    let backup = path
        .parent()
        .unwrap_or(path)
        .join(format!(".{}.contynu-backup", filename));

    if backup.exists() {
        // Restore original
        let _ = std::fs::copy(&backup, path);
        let _ = std::fs::remove_file(&backup);
    } else {
        // We created it fresh, just remove
        let _ = std::fs::remove_file(path);
    }
}

fn resolve_transport(adapter: &AdapterSpec) -> ExecutionTransport {
    let stdin_is_tty = std::io::stdin().is_terminal();
    let stdout_is_tty = std::io::stdout().is_terminal();

    if adapter.use_pty() && stdin_is_tty && stdout_is_tty {
        // Real terminal: use script-based logging to capture while preserving UX.
        ExecutionTransport::InheritTerminal
    } else if adapter.use_pty() && cfg!(unix) {
        // PTY requested and on Unix: use PTY even without a real terminal.
        // This covers configured launchers that explicitly need PTY, and
        // also non-interactive runs where the stdin prelude will be injected
        // into the PTY's stdin after a short delay.
        ExecutionTransport::Pty
    } else {
        ExecutionTransport::Pipes
    }
}

fn mirror_chunk_to_terminal(kind: StreamKind, bytes: &[u8]) -> Result<()> {
    match kind {
        StreamKind::Stdout | StreamKind::Pty => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(bytes)?;
            stdout.flush()?;
        }
        StreamKind::Stderr => {
            let mut stderr = std::io::stderr().lock();
            stderr.write_all(bytes)?;
            stderr.flush()?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn shell_command_text(executable: &OsString, args: &[OsString]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(shell_escape(&executable.to_string_lossy()));
    for arg in args {
        parts.push(shell_escape(&arg.to_string_lossy()));
    }
    parts.join(" ")
}

#[cfg(unix)]
fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn parse_transcript_log_name(
    session_id: &SessionId,
    name: &str,
) -> Option<(TurnId, TranscriptStream)> {
    let prefix = format!("{}--", session_id.as_str());
    let suffix = if name.ends_with("--stdin.log") {
        TranscriptStream::Stdin
    } else if name.ends_with("--stdout.log") {
        TranscriptStream::Stdout
    } else if name.ends_with("-stdin.log") {
        TranscriptStream::Stdin
    } else if name.ends_with("-stdout.log") {
        TranscriptStream::Stdout
    } else {
        return None;
    };

    if let Some(rest) = name.strip_prefix(&prefix) {
        let turn_text = rest
            .strip_suffix("--stdin.log")
            .or_else(|| rest.strip_suffix("--stdout.log"))?;
        return TurnId::parse(turn_text.to_string())
            .ok()
            .map(|turn_id| (turn_id, suffix));
    }

    let turn_text = name
        .strip_suffix("-stdin.log")
        .or_else(|| name.strip_suffix("-stdout.log"))?;
    TurnId::parse(turn_text.to_string())
        .ok()
        .map(|turn_id| (turn_id, suffix))
}

fn extract_interactive_dialogue(stdin_bytes: &[u8], stdout_bytes: &[u8]) -> DialogueCapture {
    DialogueCapture {
        prompts: normalize_script_prompts(stdin_bytes),
        responses: normalize_script_responses(stdout_bytes),
    }
}

fn normalize_script_prompts(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(&strip_ansi_escape_bytes(bytes)).into_owned();
    let mut prompts = Vec::new();

    for raw_line in text.replace('\r', "\n").lines() {
        let line = cleaned_terminal_line(raw_line);
        if line.is_empty() || is_script_wrapper_line(&line) {
            continue;
        }
        let prompt = strip_terminal_prefix(&line);
        if prompt.is_empty() || is_terminal_ui_line(prompt) || prompt == "/quit" {
            continue;
        }
        if looks_like_user_prompt(prompt) {
            prompts.push(one_line(prompt));
        }
    }

    dedupe_lines(prompts)
}

fn launcher_ready_for_prelude(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(&strip_ansi_escape_bytes(bytes)).into_owned();
    let compact = text.replace('\r', "\n");
    compact.contains("\n❯")
        || compact.contains("\n›")
        || compact.contains("\n >")
        || compact.contains("Type your message")
        || compact.contains("Shift+Tab to accept edits")
        || compact.contains("what can I do for you?")
}

fn normalize_script_responses(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(&strip_ansi_escape_bytes(bytes)).into_owned();
    let mut responses = Vec::<String>::new();
    let mut current = Vec::<String>::new();
    let mut capturing = false;

    for raw_line in text.replace('\r', "\n").lines() {
        let line = cleaned_terminal_line(raw_line);
        if line.is_empty() {
            if capturing
                && !current
                    .last()
                    .map(|value| value.is_empty())
                    .unwrap_or(false)
            {
                current.push(String::new());
            }
            continue;
        }
        if is_script_wrapper_line(&line) {
            continue;
        }

        if !capturing {
            if let Some(candidate) = response_start_candidate(&line) {
                capturing = true;
                if let Some((kept, boundary_hit)) = truncate_at_ui_boundary(&candidate) {
                    current.push(kept);
                    if boundary_hit {
                        finalize_response(&mut responses, &mut current);
                        capturing = false;
                    }
                }
            }
            continue;
        }

        if is_terminal_ui_line(&line)
            || line.starts_with('>')
            || line.starts_with('❯')
            || line.starts_with('›')
        {
            finalize_response(&mut responses, &mut current);
            capturing = false;
            continue;
        }

        if let Some((kept, boundary_hit)) = truncate_at_ui_boundary(&line) {
            current.push(one_line(&kept));
            if boundary_hit {
                finalize_response(&mut responses, &mut current);
                capturing = false;
            }
        }
    }

    if !current.is_empty() {
        finalize_response(&mut responses, &mut current);
    }

    dedupe_lines(responses)
}

fn finalize_response(responses: &mut Vec<String>, current: &mut Vec<String>) {
    let response = current.join("\n").trim().to_string();
    current.clear();
    if !response.is_empty() {
        responses.push(response);
    }
}

fn response_start_candidate(line: &str) -> Option<String> {
    if let Some(marked) = strip_leading_answer_marker(line) {
        return looks_like_response_text(marked).then(|| one_line(marked));
    }

    if is_terminal_ui_line(line)
        || line.starts_with('>')
        || line.starts_with('❯')
        || line.starts_with('›')
    {
        return None;
    }

    looks_like_response_text(line).then(|| one_line(line))
}

fn truncate_at_ui_boundary(line: &str) -> Option<(String, bool)> {
    let boundaries = [
        " › ",
        " ❯ ",
        " > ",
        " Token usage:",
        " To continue this session, run ",
        " Resume this session with:",
    ];

    let mut end = line.len();
    let mut boundary_hit = false;
    for boundary in boundaries {
        if let Some(index) = line.find(boundary) {
            end = end.min(index);
            boundary_hit = true;
        }
    }

    let kept = one_line(line[..end].trim());
    if kept.is_empty() {
        None
    } else {
        Some((kept, boundary_hit))
    }
}

fn strip_ansi_escape_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != 0x1b {
            out.push(bytes[index]);
            index += 1;
            continue;
        }

        index += 1;
        if index >= bytes.len() {
            break;
        }

        match bytes[index] {
            b'[' => {
                index += 1;
                while index < bytes.len() {
                    let byte = bytes[index];
                    index += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        break;
                    }
                }
            }
            b']' => {
                index += 1;
                while index < bytes.len() {
                    let byte = bytes[index];
                    index += 1;
                    if byte == 0x07 {
                        break;
                    }
                    if byte == 0x1b && index < bytes.len() && bytes[index] == b'\\' {
                        index += 1;
                        break;
                    }
                }
            }
            _ => {
                index += 1;
            }
        }
    }

    out
}

fn cleaned_terminal_line(raw_line: &str) -> String {
    raw_line
        .chars()
        .filter(|ch| {
            !matches!(*ch, '\u{0000}'..='\u{0008}' | '\u{000B}'..='\u{001F}' | '\u{007F}')
                || *ch == '\n'
                || *ch == '\t'
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn is_script_wrapper_line(line: &str) -> bool {
    line.starts_with("Script started on ") || line.starts_with("Script done on ")
}

fn strip_terminal_prefix(line: &str) -> &str {
    line.trim_start_matches(">|VTE(7600)")
        .trim_start_matches("|VTE(7600)")
        .trim_start_matches('>')
        .trim()
}

fn strip_leading_answer_marker(line: &str) -> Option<&str> {
    line.strip_prefix("• ")
        .or_else(|| line.strip_prefix("● "))
        .or_else(|| line.strip_prefix("✦ "))
        .map(str::trim)
}

fn looks_like_user_prompt(value: &str) -> bool {
    value.chars().any(|ch| ch.is_ascii_alphanumeric())
}

fn looks_like_response_text(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || is_terminal_ui_line(trimmed) {
        return false;
    }

    trimmed.chars().any(|ch| ch.is_ascii_alphabetic())
        || trimmed
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | ',' | '?' | '!' | ' '))
}

fn is_terminal_ui_line(line: &str) -> bool {
    line.starts_with('?')
        || line.starts_with("workspace ")
        || line.starts_with("sandbox")
        || line.starts_with("/model")
        || line.starts_with("model")
        || line.starts_with("Shift+Tab")
        || line.starts_with("Press Ctrl+C")
        || line.starts_with("Enable ")
        || line.starts_with("Agent powering down")
        || line.starts_with("Interaction Summary")
        || line.starts_with("To resume this session:")
        || line.starts_with("To continue this session, run")
        || line.starts_with("Token usage:")
        || line.starts_with("Tip:")
        || line.starts_with("Starting MCP servers")
        || line.starts_with("Use /skills")
        || line.starts_with("Accessing workspace:")
        || line.starts_with("Quick safety check:")
        || line.starts_with("Security guide")
        || line.starts_with("Recent activity")
        || line.starts_with("What's new")
        || line.starts_with("Waiting for authentication")
        || line.starts_with("Automatic update failed")
        || line.starts_with("Gemini CLI update available")
        || line.starts_with("Installed with npm")
        || line.starts_with("Signed in with Google:")
        || line.starts_with("Plan:")
        || line.starts_with("Claude Code v")
        || line.starts_with("OpenAI Codex")
        || line.starts_with("Gemini CLI v")
        || line.contains("Type your message or @path/to/file")
        || line.contains("GEMINI.md files")
        || line.contains("CLAUDE.md")
        || line.contains("AGENTS.md")
        || line.contains("esc to cancel")
        || line.contains("? for shortcuts")
        || line.contains("accept edits")
        || line.contains("no sandbox")
        || line.contains("Auto (Gemini")
        || line.starts_with('╭')
        || line.starts_with('╰')
        || line.starts_with('│')
        || line.contains("Rayleigh scattering") == false
            && line.chars().all(|ch| {
                matches!(
                    ch,
                    '─' | '▀'
                        | '▄'
                        | '│'
                        | '╭'
                        | '╰'
                        | '▝'
                        | '▜'
                        | '▗'
                        | '▟'
                        | '▘'
                        | ' '
                        | '█'
                        | '▌'
                        | '▛'
                        | '▖'
                        | '▚'
                        | '▞'
                        | '▐'
                )
            })
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn dedupe_lines(lines: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for line in lines {
        if deduped.last() != Some(&line) {
            deduped.push(line);
        }
    }
    deduped
}

/// Extract the meaningful text from stdout_captured events, filtering UI noise.
fn extract_captured_response(events: &[EventRecord]) -> Option<String> {
    let mut parts = Vec::new();
    for event in events {
        if event.event_type == "stdout_captured" || event.event_type == "message_output" {
            if let Some(text) = event
                .payload_json
                .get("text")
                .and_then(|v| v.as_str())
            {
                // Filter out lines that are terminal UI noise
                let clean: Vec<&str> = text
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .filter(|l| !is_terminal_ui_line(l))
                    .filter(|l| l.chars().any(|c| c.is_alphabetic()))
                    .collect();
                if !clean.is_empty() {
                    parts.push(clean.join(" "));
                }
            }
        }
    }
    let combined = parts.join(" ");
    if combined.is_empty() { None } else { Some(combined) }
}

fn latest_dialogue_from_events(events: &[EventRecord]) -> Option<(String, String)> {
    let mut prompts = Vec::new();
    let mut responses = Vec::new();

    for event in events {
        match event.event_type.as_str() {
            "stdin_captured" | "message_input" => {
                if let Some(text) = extract_event_text(event) {
                    let text = one_line(text.trim());
                    if !text.is_empty() {
                        prompts.push(text);
                    }
                }
            }
            "stdout_captured" | "message_output" => {
                if let Some(text) = extract_event_text(event) {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        responses.push(text);
                    }
                }
            }
            _ => {}
        }
    }

    match (prompts.last(), responses.last()) {
        (Some(prompt), Some(response)) => Some((prompt.clone(), one_line(response))),
        _ => None,
    }
}

fn derive_structured_candidates(
    events: &[EventRecord],
    exit_code: Option<i32>,
) -> Vec<MemoryCandidate> {
    let mut candidates = Vec::new();
    for event in events {
        if let Some(text) = extract_event_text(event) {
            let lines: Vec<&str> = text.lines().map(|l| l.trim()).collect();

            // Strategy 1: Explicit prefix matching (highest confidence)
            for line in &lines {
                if line.is_empty() {
                    continue;
                }
                if let Some(value) = strip_prefix_case_insensitive(line, "fact:") {
                    candidates.push(MemoryCandidate {
                        kind: MemoryObjectKind::Fact,
                        text: value.to_string(),
                        confidence: 0.95,
                    });
                } else if let Some(value) = strip_prefix_case_insensitive(line, "constraint:") {
                    candidates.push(MemoryCandidate {
                        kind: MemoryObjectKind::Constraint,
                        text: value.to_string(),
                        confidence: 0.95,
                    });
                } else if let Some(value) = strip_prefix_case_insensitive(line, "decision:") {
                    candidates.push(MemoryCandidate {
                        kind: MemoryObjectKind::Decision,
                        text: value.to_string(),
                        confidence: 0.95,
                    });
                } else if let Some(value) = strip_prefix_case_insensitive(line, "todo:") {
                    candidates.push(MemoryCandidate {
                        kind: MemoryObjectKind::Todo,
                        text: value.to_string(),
                        confidence: 0.9,
                    });
                }
            }

            // Strategy 2: Markdown header sections (### Facts, ### Decisions, etc.)
            let mut current_header_kind: Option<MemoryObjectKind> = None;
            for line in &lines {
                if let Some(header) = line.strip_prefix("### ").or_else(|| line.strip_prefix("## ")) {
                    let lower = header.to_ascii_lowercase();
                    current_header_kind = if lower.starts_with("fact") {
                        Some(MemoryObjectKind::Fact)
                    } else if lower.starts_with("constraint") {
                        Some(MemoryObjectKind::Constraint)
                    } else if lower.starts_with("decision") {
                        Some(MemoryObjectKind::Decision)
                    } else if lower.starts_with("todo") || lower.starts_with("task") || lower.starts_with("open") {
                        Some(MemoryObjectKind::Todo)
                    } else {
                        None
                    };
                } else if let Some(kind) = current_header_kind {
                    if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
                        let item = item.trim();
                        if !item.is_empty() {
                            candidates.push(MemoryCandidate {
                                kind,
                                text: item.to_string(),
                                confidence: 0.90,
                            });
                        }
                    } else if line.is_empty() || line.starts_with('#') {
                        current_header_kind = None;
                    }
                }
            }

            // Strategy 3: Key phrase detection (lower confidence)
            for line in &lines {
                if line.is_empty() || line.len() < 10 {
                    continue;
                }
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("we decided") || lower.starts_with("the decision was") || lower.starts_with("decided to ") {
                    candidates.push(MemoryCandidate {
                        kind: MemoryObjectKind::Decision,
                        text: line.to_string(),
                        confidence: 0.75,
                    });
                } else if lower.contains("must ") && (lower.contains("always") || lower.contains("never")) {
                    candidates.push(MemoryCandidate {
                        kind: MemoryObjectKind::Constraint,
                        text: line.to_string(),
                        confidence: 0.75,
                    });
                } else if lower.starts_with("note:") || lower.starts_with("important:") || lower.starts_with("remember:") {
                    let value = line.splitn(2, ':').nth(1).unwrap_or("").trim();
                    if !value.is_empty() {
                        candidates.push(MemoryCandidate {
                            kind: MemoryObjectKind::Fact,
                            text: value.to_string(),
                            confidence: 0.75,
                        });
                    }
                } else if lower.starts_with("still need to ") || lower.starts_with("next step") || lower.starts_with("remaining:") {
                    candidates.push(MemoryCandidate {
                        kind: MemoryObjectKind::Todo,
                        text: line.to_string(),
                        confidence: 0.75,
                    });
                }
            }
        }
    }

    if exit_code.unwrap_or_default() != 0 {
        for event in events.iter().rev() {
            if event.event_type == "stderr_captured" || event.event_type == "stdout_captured" {
                if let Some(text) = event
                    .payload_json
                    .get("text")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    candidates.push(MemoryCandidate {
                        kind: MemoryObjectKind::Fact,
                        text: format!(
                            "Last failure output: {}",
                            text.lines().next().unwrap_or(text)
                        ),
                        confidence: 0.75,
                    });
                    break;
                }
            }
        }
    }

    dedupe_candidates(candidates)
}

fn extract_event_text(event: &EventRecord) -> Option<String> {
    match event.event_type.as_str() {
        "stdout_captured" | "stderr_captured" | "stdin_captured" => event
            .payload_json
            .get("text")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        "message_input" | "message_output" | "message_chunk" => event
            .payload_json
            .get("content")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("text").and_then(|value| value.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|value| !value.is_empty()),
        _ => None,
    }
}

fn strip_prefix_case_insensitive<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with(prefix) {
        Some(value[prefix.len()..].trim())
    } else {
        None
    }
}

fn dedupe_candidates(candidates: Vec<MemoryCandidate>) -> Vec<MemoryCandidate> {
    let mut seen = std::collections::BTreeSet::new();
    let mut deduped = Vec::new();
    for candidate in candidates {
        let key = format!("{}:{}", candidate.kind.as_str(), candidate.text);
        if seen.insert(key) {
            deduped.push(candidate);
        }
    }
    deduped
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    kind: StreamKind,
    sender: Sender<StreamMessage>,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || loop {
        let mut buffer = vec![0_u8; STREAM_CHUNK_SIZE];
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(error)
                if matches!(kind, StreamKind::Pty) && error.raw_os_error() == Some(libc::EIO) =>
            {
                let _ = sender.send(StreamMessage::Closed { kind });
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
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

#[cfg(unix)]
fn install_pty_ctrlc_handler(child: &PtyChild, interrupted: Arc<AtomicBool>) {
    let pid = child.pid();
    ctrlc::set_handler(move || {
        interrupted.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
    })
    .ok();
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use chrono::Utc;
    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        derive_structured_candidates, extract_interactive_dialogue, RunConfig, RuntimeEngine,
    };
    use crate::store::EventRecord;
    use crate::{EventId, Journal, MemoryObjectKind, MetadataStore, ProjectId, StatePaths, TurnId};

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

        let memory = store
            .list_memory_objects(&outcome.project_id, None)
            .unwrap();
        assert!(memory
            .iter()
            .any(|item| item.kind == MemoryObjectKind::Summary));
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
    fn interactive_dialogue_extraction_keeps_only_clean_prompts_and_responses() {
        let stdin = "Script started on 2026-04-02
>|VTE(7600)why is the sky blue?
what was the answer again?
Script done on 2026-04-02
";
        let stdout = "Script started on 2026-04-02
╭────────────────────────╮
│ >_ OpenAI Codex       │
╰────────────────────────╯
› why is the sky blue?
• The sky appears blue because shorter blue wavelengths scatter more strongly in the atmosphere.
Token usage: total=12
› what was the answer again?
• Blue light scatters more strongly than red light, so the sky looks blue. › Summarize recent commits
To continue this session, run codex resume 123
Script done on 2026-04-02
";

        let dialogue = extract_interactive_dialogue(stdin.as_bytes(), stdout.as_bytes());
        assert_eq!(
            dialogue.prompts,
            vec![
                "why is the sky blue?".to_string(),
                "what was the answer again?".to_string()
            ]
        );
        assert_eq!(
            dialogue.responses,
            vec![
                "The sky appears blue because shorter blue wavelengths scatter more strongly in the atmosphere.".to_string(),
                "Blue light scatters more strongly than red light, so the sky looks blue.".to_string()
            ]
        );
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
        if !cfg!(unix) {
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
        assert!(events
            .iter()
            .any(|event| event.event_type == "stdout_captured"));
    }

    #[test]
    fn structured_candidates_extract_marked_memory() {
        let event = EventRecord {
            event_id: EventId::new(),
            session_id: ProjectId::new(),
            turn_id: Some(TurnId::new()),
            seq: 1,
            ts: Utc::now(),
            actor: "assistant".into(),
            event_type: "message_output".into(),
            payload_json: json!({
                "content": [
                    {"text": "Fact: sqlite is canonical metadata\nConstraint: keep journal append-only\nDecision: use project-first continuity\nTodo: improve signal handling"}
                ]
            }),
            checksum: "sha256:test".into(),
            journal_path: "journal".into(),
            journal_byte_offset: 0,
            journal_line: 1,
        };

        let candidates = derive_structured_candidates(&[event], Some(0));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryObjectKind::Fact));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryObjectKind::Constraint));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryObjectKind::Decision));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryObjectKind::Todo));
    }
}
