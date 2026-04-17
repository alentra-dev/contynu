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

use crate::adapters::{AdapterSpec, HydrationContext};
use crate::blobs::BlobStore;
use crate::checkpoint::{sanitize_packet, CheckpointManager, PacketBudget};
use crate::config::ContynuConfig;
use crate::error::{ContynuError, Result};
use crate::ids::{ProjectId, SessionId};
use crate::pty::PtyChild;
use crate::state::StatePaths;
use crate::store::{MetadataStore, SessionRecord};

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

#[derive(Debug, Clone, Copy)]
enum ExecutionTransport {
    Pipes,
    Pty,
    InheritTerminal,
}

#[derive(Debug)]
struct ProcessCapture {
    exit_code: Option<i32>,
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

        // Clean up old architecture artifacts on first run
        let _ = state.cleanup_old_architecture();

        let store = MetadataStore::open(state.sqlite_db())?;
        let config_file = ContynuConfig::load(&state.config_path())?;
        let blob_store = BlobStore::new(state.blobs_root());

        // If a previous run crashed before restoring the user's files, put
        // them back before we write our merged copies. Keeps the working tree
        // clean across model swaps even when something went wrong last time.
        let _ = reconcile_orphan_context_backups(&state, &config.cwd);
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

        // Set environment variables so the MCP server and child process know the context
        std::env::set_var("CONTYNU_ACTIVE_PROJECT", session_id.as_str());
        std::env::set_var("CONTYNU_STATE_DIR", config.state_dir.display().to_string());

        let hydration_budget = config_file.packet_budget.to_budget();
        let hydration = if adapter.should_hydrate() && continuing_session {
            Some(Self::prepare_hydration(
                &state,
                &store,
                &blob_store,
                &session_id,
                &adapter,
                &hydration_budget,
            )?)
        } else {
            None
        };

        let launch_plan = adapter.build_launch_plan(
            config.command[0].clone(),
            config.command[1..].to_vec(),
            hydration.as_ref(),
        )?;

        // Write context files for adapters that read project instructions from files.
        let context_file = if let Some(ref hydration) = hydration {
            write_context_file(&state, &config.cwd, &adapter, &hydration.prompt_text)?
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
            &session_id,
            Arc::clone(&interrupted),
        )?;

        // Restore original context file after execution.
        if let Some(ref path) = context_file {
            cleanup_context_file(&state, path);
        }

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
            let _ =
                manager.create_checkpoint(&session_id, "run_completed", None, &hydration_budget)?;
        }

        Ok(RunOutcome {
            project_id: session_id,
            exit_code: capture.exit_code,
            interrupted: interrupted.load(Ordering::SeqCst),
        })
    }

    fn execute_launch_plan(
        state: &StatePaths,
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
        transport: ExecutionTransport,
        session_id: &SessionId,
        interrupted: Arc<AtomicBool>,
    ) -> Result<ProcessCapture> {
        match transport {
            ExecutionTransport::Pipes => Self::execute_with_pipes(cwd, launch_plan, interrupted),
            ExecutionTransport::InheritTerminal => {
                #[cfg(unix)]
                {
                    Self::execute_with_script_logging(
                        state,
                        cwd,
                        launch_plan,
                        session_id,
                        interrupted,
                    )
                }
                #[cfg(not(unix))]
                {
                    let _ = (state, session_id);
                    Self::execute_with_inherited_terminal(cwd, launch_plan, interrupted)
                }
            }
            ExecutionTransport::Pty => {
                #[cfg(unix)]
                {
                    Self::execute_with_pty(cwd, launch_plan, interrupted)
                }
                #[cfg(not(unix))]
                {
                    Self::execute_with_pipes(cwd, launch_plan, interrupted)
                }
            }
        }
    }

    fn execute_with_pipes(
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
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
            .map_err(classify_spawn_error)?;

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

        drain_streams(receiver)?;
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
            exit_code: status.code(),
        })
    }

    #[allow(dead_code)]
    fn execute_with_inherited_terminal(
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
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
            .map_err(classify_spawn_error)?;

        let child = Arc::new(Mutex::new(child));
        install_ctrlc_handler(Arc::clone(&child), interrupted);
        let status = child
            .lock()
            .map_err(|_| ContynuError::Validation("child process mutex poisoned".into()))?
            .wait()?;

        Ok(ProcessCapture {
            exit_code: status.code(),
        })
    }

    #[cfg(unix)]
    fn execute_with_pty(
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
        interrupted: Arc<AtomicBool>,
    ) -> Result<ProcessCapture> {
        let child = PtyChild::spawn(
            cwd,
            &launch_plan.executable,
            &launch_plan.args,
            &launch_plan.env,
        )?;
        let mut stdin = child.try_clone_writer()?;
        let (prelude_sender, prelude_receiver) = mpsc::channel::<()>();
        let stdin_prelude = launch_plan.stdin_prelude.clone();
        let stdin_is_tty = std::io::stdin().is_terminal();
        let stdin_handle = thread::spawn(move || {
            if let Some(stdin_prelude) = stdin_prelude.as_ref() {
                if stdin_is_tty {
                    match prelude_receiver.recv_timeout(std::time::Duration::from_secs(8)) {
                        Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                if stdin.write_all(stdin_prelude).is_ok() {
                    let _ = stdin.flush();
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
                    }
                    Err(_) => break,
                }
            }
            let _ = stdin.flush();
        });

        let mut stdout = child.try_clone_reader()?;
        install_pty_ctrlc_handler(&child, interrupted);
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

        Ok(ProcessCapture {
            exit_code: status.code(),
        })
    }

    #[cfg(unix)]
    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
    fn execute_with_script_logging(
        _state: &StatePaths,
        cwd: &std::path::Path,
        launch_plan: &crate::adapters::LaunchPlan,
        _session_id: &SessionId,
        interrupted: Arc<AtomicBool>,
    ) -> Result<ProcessCapture> {
        let command_text = shell_command_text(&launch_plan.executable, &launch_plan.args);
        let mut command = Command::new("script");
        command.current_dir(cwd);
        command.stdin(Stdio::inherit());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.envs(launch_plan.env.iter().map(|(key, value)| (key, value)));
        command.arg("-qefc");
        command.arg(command_text);

        let mut child = command
            .spawn()
            .map_err(classify_spawn_error)?;

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
        drain_streams_with_indicator(receiver, &mut handoff_indicator)?;
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

        Ok(ProcessCapture {
            exit_code: status.code(),
        })
    }

    fn prepare_hydration(
        state: &StatePaths,
        store: &MetadataStore,
        blob_store: &BlobStore,
        project_id: &ProjectId,
        adapter: &AdapterSpec,
        budget: &PacketBudget,
    ) -> Result<HydrationContext> {
        let manager = CheckpointManager::new(state, store, blob_store);
        let packet = manager.build_packet_with_budget(project_id, None, budget)?;
        let runtime_dir = state.project_runtime_dir(project_id);
        std::fs::create_dir_all(&runtime_dir)?;
        let packet_path = runtime_dir.join("rehydration.json");
        let prompt_path = runtime_dir.join("rehydration.txt");
        let packet_json = serde_json::to_string_pretty(&sanitize_packet(&packet))?;
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

/// Filename + marker-name for each adapter that uses a working-directory
/// context file (Codex/Claude/Gemini auto-read these). The marker keeps our
/// injected block identifiable across model swaps so we can merge / unmerge
/// without touching the user's own content.
const CONTEXT_FILES: &[(crate::AdapterKind, &str, &str)] = &[
    (crate::AdapterKind::CodexCli, "AGENTS.md", "codex"),
    (crate::AdapterKind::ClaudeCli, "CLAUDE.md", "claude"),
    (crate::AdapterKind::GeminiCli, "GEMINI.md", "gemini"),
];

fn context_file_spec(kind: crate::AdapterKind) -> Option<(&'static str, &'static str)> {
    CONTEXT_FILES
        .iter()
        .find(|(k, _, _)| *k == kind)
        .map(|(_, filename, marker)| (*filename, *marker))
}

fn context_backup_root(state: &StatePaths) -> std::path::PathBuf {
    // Kept outside `runtime/` on purpose: `cleanup_old_architecture` wipes
    // `runtime/` on every run, and losing a crash-recovery backup would
    // permanently clobber the user's original file.
    state.root().join("context-backups")
}

fn backup_paths(state: &StatePaths, filename: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = context_backup_root(state);
    (
        dir.join(format!("{filename}.backup")),
        dir.join(format!("{filename}.no-original")),
    )
}

fn merged_context(prompt_text: &str, marker: &str, existing: &str) -> String {
    format!(
        "<!-- contynu:{marker}:start -->\n{prompt_text}\n<!-- contynu:{marker}:end -->\n\n## Repository Instructions\n\nFollow the repository instructions below together with the carried-forward working state above.\n\n{existing}"
    )
}

fn standalone_context(prompt_text: &str, marker: &str) -> String {
    format!("<!-- contynu:{marker}:start -->\n{prompt_text}\n<!-- contynu:{marker}:end -->\n")
}

/// Write a context file (AGENTS.md, CLAUDE.md, GEMINI.md) in the working
/// directory so LLM CLIs pick up continuity automatically. The user's
/// original file — if any — is copied into the state dir so crashes never
/// pollute the working tree.
fn write_context_file(
    state: &StatePaths,
    cwd: &std::path::Path,
    adapter: &AdapterSpec,
    prompt_text: &str,
) -> crate::error::Result<Option<std::path::PathBuf>> {
    let Some((filename, marker)) = context_file_spec(adapter.kind()) else {
        return Ok(None);
    };

    let path = cwd.join(filename);
    let backup_dir = context_backup_root(state);
    std::fs::create_dir_all(&backup_dir)?;
    let (backup_path, no_original_path) = backup_paths(state, filename);

    // Clear stale sentinels from a prior run before taking a new snapshot.
    let _ = std::fs::remove_file(&backup_path);
    let _ = std::fs::remove_file(&no_original_path);

    if path.exists() {
        std::fs::copy(&path, &backup_path)?;
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(&path, merged_context(prompt_text, marker, &existing))?;
    } else {
        // Sentinel: we created the file, so restore means "delete it".
        std::fs::write(&no_original_path, b"")?;
        std::fs::write(&path, standalone_context(prompt_text, marker))?;
    }

    Ok(Some(path))
}

fn cleanup_context_file(state: &StatePaths, path: &std::path::Path) {
    let Some(filename) = path.file_name().and_then(|f| f.to_str()) else {
        return;
    };
    let (backup_path, no_original_path) = backup_paths(state, filename);

    if no_original_path.exists() {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(&no_original_path);
    } else if backup_path.exists() {
        let _ = std::fs::rename(&backup_path, path);
    }
}

/// If a previous run crashed before `cleanup_context_file` ran, the user is
/// left with our merged file in the working tree and a sentinel in the state
/// dir. On the next startup, undo the merge before doing anything else so the
/// user never has to clean up after us.
fn reconcile_orphan_context_backups(state: &StatePaths, cwd: &std::path::Path) {
    for (_, filename, marker) in CONTEXT_FILES {
        let path = cwd.join(filename);
        let (backup_path, no_original_path) = backup_paths(state, filename);
        let had_backup = backup_path.exists();
        let had_no_original = no_original_path.exists();
        if !had_backup && !had_no_original {
            continue;
        }

        let current = std::fs::read_to_string(&path).unwrap_or_default();
        let start_marker = format!("contynu:{marker}:start");

        if current.contains(&start_marker) {
            if had_no_original {
                let _ = std::fs::remove_file(&path);
            } else {
                let _ = std::fs::rename(&backup_path, &path);
            }
        }

        // Whether we restored or the user hand-fixed things, clear the
        // sentinels so the next run starts from a clean slate.
        let _ = std::fs::remove_file(&backup_path);
        let _ = std::fs::remove_file(&no_original_path);
    }
}

fn classify_spawn_error(error: std::io::Error) -> ContynuError {
    // `ArgumentListTooLong` means argv+envp exceeded the OS ARG_MAX. We keep
    // the injected prompt out of argv on purpose, but an adapter config or
    // oversized env could still trip this. Surface a self-diagnosing message
    // so continuity failures never look cryptic.
    if error.kind() == std::io::ErrorKind::ArgumentListTooLong {
        return ContynuError::CommandStart(
            "rehydration context exceeded the OS argv/env limit. \
             This should not happen with built-in launchers; if you have a \
             custom launcher in config.json, move large context into a file \
             via `{prompt_file}` instead of `{prompt_text}`."
                .into(),
        );
    }
    ContynuError::CommandStart(error.to_string())
}

fn resolve_transport(adapter: &AdapterSpec) -> ExecutionTransport {
    let stdin_is_tty = std::io::stdin().is_terminal();
    let stdout_is_tty = std::io::stdout().is_terminal();

    if adapter.use_pty() && stdin_is_tty && stdout_is_tty {
        ExecutionTransport::InheritTerminal
    } else if adapter.use_pty() && cfg!(unix) {
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

/// Drain all stream messages, mirroring to terminal.
fn drain_streams(receiver: Receiver<StreamMessage>) -> Result<()> {
    let mut stdout_closed = false;
    let mut stderr_closed = false;

    while !(stdout_closed && stderr_closed) {
        match receiver
            .recv()
            .map_err(|_| ContynuError::InvalidState("stream capture channel closed".into()))?
        {
            StreamMessage::Chunk { kind, bytes } => {
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

/// Drain all stream messages with a startup indicator.
#[allow(dead_code)]
fn drain_streams_with_indicator(
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
    use tempfile::tempdir;

    use super::{
        backup_paths, reconcile_orphan_context_backups, write_context_file, RunConfig,
        RuntimeEngine,
    };
    use crate::adapters::AdapterSpec;
    use crate::config::ContynuConfig;
    use crate::{MetadataStore, StatePaths};

    #[test]
    fn context_file_backs_up_into_state_dir_and_restores() {
        let dir = tempdir().unwrap();
        let cwd = dir.path().join("cwd");
        let state_dir = dir.path().join(".contynu");
        std::fs::create_dir_all(&cwd).unwrap();
        let state = StatePaths::new(&state_dir);
        state.ensure_layout().unwrap();
        std::fs::write(cwd.join("CLAUDE.md"), "user's own guidance").unwrap();

        let adapter = AdapterSpec::detect("claude", &ContynuConfig::default());
        let path = write_context_file(&state, &cwd, &adapter, "CONTINUITY_BLOCK")
            .unwrap()
            .unwrap();

        let merged = std::fs::read_to_string(&path).unwrap();
        assert!(merged.contains("contynu:claude:start"));
        assert!(merged.contains("CONTINUITY_BLOCK"));
        assert!(merged.contains("user's own guidance"));
        // Backup must not leak into the working tree.
        assert!(!cwd.join(".CLAUDE.md.contynu-backup").exists());
        let (backup, _) = backup_paths(&state, "CLAUDE.md");
        assert!(backup.exists());

        super::cleanup_context_file(&state, &path);
        let restored = std::fs::read_to_string(&path).unwrap();
        assert_eq!(restored, "user's own guidance");
        assert!(!backup.exists());
    }

    #[test]
    fn context_file_without_original_is_removed_on_cleanup() {
        let dir = tempdir().unwrap();
        let cwd = dir.path().join("cwd");
        let state_dir = dir.path().join(".contynu");
        std::fs::create_dir_all(&cwd).unwrap();
        let state = StatePaths::new(&state_dir);
        state.ensure_layout().unwrap();

        let adapter = AdapterSpec::detect("claude", &ContynuConfig::default());
        let path = write_context_file(&state, &cwd, &adapter, "CONTINUITY_BLOCK")
            .unwrap()
            .unwrap();
        assert!(path.exists());
        let (_, no_original) = backup_paths(&state, "CLAUDE.md");
        assert!(no_original.exists());

        super::cleanup_context_file(&state, &path);
        assert!(!path.exists());
        assert!(!no_original.exists());
    }

    #[test]
    fn reconcile_restores_orphan_backup_from_prior_crash() {
        let dir = tempdir().unwrap();
        let cwd = dir.path().join("cwd");
        let state_dir = dir.path().join(".contynu");
        std::fs::create_dir_all(&cwd).unwrap();
        let state = StatePaths::new(&state_dir);
        state.ensure_layout().unwrap();
        std::fs::write(cwd.join("CLAUDE.md"), "user's own guidance").unwrap();

        let adapter = AdapterSpec::detect("claude", &ContynuConfig::default());
        let _ = write_context_file(&state, &cwd, &adapter, "CONTINUITY_BLOCK")
            .unwrap()
            .unwrap();
        // Simulate a crash before cleanup ran: merged file + backup both exist.

        reconcile_orphan_context_backups(&state, &cwd);

        let restored = std::fs::read_to_string(cwd.join("CLAUDE.md")).unwrap();
        assert_eq!(restored, "user's own guidance");
        let (backup, no_original) = backup_paths(&state, "CLAUDE.md");
        assert!(!backup.exists());
        assert!(!no_original.exists());
    }

    #[test]
    fn reconcile_removes_contynu_only_file_when_user_had_no_original() {
        let dir = tempdir().unwrap();
        let cwd = dir.path().join("cwd");
        let state_dir = dir.path().join(".contynu");
        std::fs::create_dir_all(&cwd).unwrap();
        let state = StatePaths::new(&state_dir);
        state.ensure_layout().unwrap();

        let adapter = AdapterSpec::detect("claude", &ContynuConfig::default());
        let _ = write_context_file(&state, &cwd, &adapter, "CONTINUITY_BLOCK")
            .unwrap()
            .unwrap();
        // Simulated crash: CLAUDE.md + no-original sentinel both present.

        reconcile_orphan_context_backups(&state, &cwd);
        assert!(!cwd.join("CLAUDE.md").exists());
    }

    #[test]
    fn reconcile_leaves_user_edited_file_alone() {
        let dir = tempdir().unwrap();
        let cwd = dir.path().join("cwd");
        let state_dir = dir.path().join(".contynu");
        std::fs::create_dir_all(&cwd).unwrap();
        let state = StatePaths::new(&state_dir);
        state.ensure_layout().unwrap();
        std::fs::write(cwd.join("CLAUDE.md"), "user's own guidance").unwrap();

        let adapter = AdapterSpec::detect("claude", &ContynuConfig::default());
        let _ = write_context_file(&state, &cwd, &adapter, "CONTINUITY_BLOCK")
            .unwrap()
            .unwrap();
        // User manually restored their file between the crash and next startup.
        std::fs::write(cwd.join("CLAUDE.md"), "user hand-fixed it").unwrap();

        reconcile_orphan_context_backups(&state, &cwd);

        let content = std::fs::read_to_string(cwd.join("CLAUDE.md")).unwrap();
        assert_eq!(content, "user hand-fixed it");
        let (backup, no_original) = backup_paths(&state, "CLAUDE.md");
        assert!(!backup.exists());
        assert!(!no_original.exists());
    }

    #[test]
    fn runtime_run_executes_process() {
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
            checkpoint_on_exit: false,
            project_id: None,
        })
        .unwrap();

        assert_eq!(outcome.exit_code, Some(0));
        assert!(workspace.join("output.txt").exists());

        // Verify session was registered
        let paths = StatePaths::new(state);
        let store = MetadataStore::open(paths.sqlite_db()).unwrap();
        let session = store.get_session(&outcome.project_id).unwrap();
        assert!(session.is_some());
    }

    #[test]
    fn runtime_can_continue_existing_session() {
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
    }
}
