mod mcp_registration;
mod mcp_server;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand};
use contynu_core::{
    BlobStore, CheckpointManager, ContynuConfig, EventDraft, EventId, EventType, Journal,
    MetadataStore, ProjectId, RunConfig, RunOutcome, RuntimeEngine, StatePaths,
};
use serde_json::Value;

#[derive(Debug, Parser)]
#[command(name = "contynu")]
#[command(about = "Local-first continuity engine for LLM workflows")]
struct Cli {
    #[arg(long, global = true, default_value = ".contynu")]
    state_dir: PathBuf,

    #[arg(long, global = true, default_value = ".")]
    cwd: PathBuf,

    #[arg(long, global = true)]
    new: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    Run(RunCommand),
    Codex(LlmCommand),
    Claude(LlmCommand),
    Gemini(LlmCommand),
    Status {
        #[arg(long, alias = "session")]
        project: Option<String>,
    },
    Projects,
    Recent {
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    #[command(name = "start-project", visible_alias = "start-session")]
    StartProject,
    Checkpoint {
        #[arg(long, alias = "session")]
        project: Option<String>,
        #[arg(long, default_value = "manual")]
        reason: String,
    },
    Resume {
        #[arg(long, alias = "session")]
        project: Option<String>,
    },
    Handoff {
        #[arg(long, alias = "session")]
        project: Option<String>,
        #[arg(long)]
        target_model: String,
    },
    Replay {
        #[arg(long, alias = "session")]
        project: Option<String>,
    },
    Inspect {
        #[command(subcommand)]
        command: InspectCommand,
    },
    Search {
        #[command(subcommand)]
        command: SearchCommand,
    },
    Artifacts {
        #[command(subcommand)]
        command: ArtifactsCommand,
    },
    Doctor,
    Repair {
        #[arg(long, alias = "session")]
        project: Option<String>,
    },
    /// Ingest events from stdin (JSONL) into the project journal
    Ingest {
        #[arg(long, alias = "session")]
        project: Option<String>,
        #[arg(long)]
        adapter: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value_t = true)]
        derive_memory: bool,
    },
    /// Import conversations from Claude JSONL, ChatGPT JSON, or plain text
    Import {
        /// Path to conversation file(s)
        #[arg(required = true)]
        files: Vec<std::path::PathBuf>,
        #[arg(long, default_value = "auto")]
        format: String,
        #[arg(long, alias = "session")]
        project: Option<String>,
        #[arg(long)]
        adapter: Option<String>,
    },
    /// Export importance-ranked memories as Markdown
    #[command(name = "export-memory")]
    ExportMemory {
        #[arg(long, alias = "session")]
        project: Option<String>,
        #[arg(long, default_value_t = 20000)]
        max_chars: usize,
        #[arg(long)]
        with_markers: bool,
    },
    /// Configure Contynu integration with OpenClaw
    #[command(name = "openclaw")]
    OpenClaw {
        #[command(subcommand)]
        command: OpenClawCommand,
    },
    /// Start the Contynu MCP server (stdio transport)
    #[command(name = "mcp-server")]
    McpServer {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

#[derive(Debug, Args)]
struct RunCommand {
    #[arg(long, alias = "session")]
    project: Option<String>,

    #[arg(long)]
    no_checkpoint: bool,

    #[arg(long = "ignore")]
    ignore_patterns: Vec<String>,

    #[arg(last = true, required = true)]
    command: Vec<String>,
}

#[derive(Debug, Args)]
struct LlmCommand {
    #[arg(long, alias = "session")]
    project: Option<String>,

    #[arg(long)]
    no_checkpoint: bool,

    #[arg(long = "ignore")]
    ignore_patterns: Vec<String>,

    #[arg(trailing_var_arg = true)]
    args: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum OpenClawCommand {
    /// Auto-configure Contynu for use with OpenClaw
    Setup {
        #[arg(long)]
        openclaw_config: Option<std::path::PathBuf>,
    },
    /// Show OpenClaw integration status
    Status,
}

#[derive(Debug, Subcommand)]
enum InspectCommand {
    #[command(name = "project", visible_alias = "session")]
    Project {
        id: Option<String>,
    },
    Event {
        id: String,
    },
}

#[derive(Debug, Subcommand)]
enum SearchCommand {
    Exact { query: String },
    Memory { query: String },
}

#[derive(Debug, Subcommand)]
enum ArtifactsCommand {
    List {
        #[arg(long, alias = "session")]
        project: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Validate,
    Show,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let state = StatePaths::new(&cli.state_dir);
    maybe_reset_state_for_new(&state, cli.command.as_ref(), cli.new)?;

    match cli.command {
        Some(Command::Init) => init(&state),
        Some(Command::Run(command)) => run(&state, &cli.cwd, command),
        Some(Command::Codex(command)) => launch_llm(&state, &cli.cwd, "codex", command),
        Some(Command::Claude(command)) => launch_llm(&state, &cli.cwd, "claude", command),
        Some(Command::Gemini(command)) => launch_llm(&state, &cli.cwd, "gemini", command),
        Some(Command::Status { project }) => status(&state, project.as_deref()),
        Some(Command::Projects) => projects(&state),
        Some(Command::Recent { limit }) => recent(&state, limit),
        Some(Command::Config { command }) => config_command(&state, command),
        Some(Command::StartProject) => start_project(&state, &cli.cwd),
        Some(Command::Checkpoint { project, reason }) => {
            checkpoint(&state, project.as_deref(), &reason)
        }
        Some(Command::Resume { project }) => resume(&state, project.as_deref(), None),
        Some(Command::Handoff {
            project,
            target_model,
        }) => resume(&state, project.as_deref(), Some(target_model)),
        Some(Command::Replay { project }) => replay(&state, project.as_deref()),
        Some(Command::Inspect { command }) => inspect(&state, command),
        Some(Command::Search { command }) => search(&state, command),
        Some(Command::Artifacts { command }) => artifacts(&state, command),
        Some(Command::Doctor) => doctor(&state),
        Some(Command::OpenClaw { command }) => openclaw_command(&state, &cli.cwd, command),
        Some(Command::Import { files, format, project, adapter }) => {
            import_conversations(&state, project.as_deref(), &files, &format, adapter.as_deref())
        }
        Some(Command::Ingest { project, adapter, model, derive_memory }) => {
            ingest(&state, project.as_deref(), adapter.as_deref(), model.as_deref(), derive_memory)
        }
        Some(Command::ExportMemory { project, max_chars, with_markers }) => {
            export_memory(&state, project.as_deref(), max_chars, with_markers)
        }
        Some(Command::Repair { project }) => repair(&state, project.as_deref()),
        Some(Command::McpServer { state_dir: override_dir }) => {
            let dir = override_dir.unwrap_or_else(|| {
                std::env::var("CONTYNU_STATE_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| cli.state_dir.clone())
            });
            mcp_server::run(&dir)
        }
        Some(Command::External(command)) => passthrough(&state, &cli.cwd, command),
        None if cli.new => {
            println!("Started fresh. Launch a tool with `contynu codex`, `contynu claude`, `contynu gemini`, or `contynu <command...>`.");
            Ok(())
        }
        None => Err(anyhow!(
            "a command is required; run `contynu --help` for available subcommands"
        )),
    }
}

fn maybe_reset_state_for_new(
    state: &StatePaths,
    command: Option<&Command>,
    reset: bool,
) -> Result<()> {
    if !reset {
        return Ok(());
    }

    if let Some(command) = command.filter(|command| {
        !matches!(
            command,
            Command::Run(_)
                | Command::Codex(_)
                | Command::Claude(_)
                | Command::Gemini(_)
                | Command::StartProject
                | Command::External(_)
        )
    }) {
        let _ = command;
        return Err(anyhow!(
            "`--new` is only supported when starting a new run or launcher session"
        ));
    }

    eprintln!(
        "Contynu will permanently wipe the chat history for this project folder at {}.",
        state.root().display()
    );
    eprint!("Type `yes` to continue: ");
    io::stderr().flush()?;

    let mut confirmation = String::new();
    io::stdin().read_line(&mut confirmation)?;
    if confirmation.trim() != "yes" {
        return Err(anyhow!("aborted without wiping project history"));
    }

    if state.root().exists() {
        std::fs::remove_dir_all(state.root())?;
    }

    Ok(())
}

fn init(state: &StatePaths) -> Result<()> {
    ensure_state(state)?;
    println!("Contynu is ready at {}.", state.root().display());
    println!("Use `contynu codex`, `contynu claude`, `contynu gemini`, or `contynu <command...>` to keep working.");
    Ok(())
}

fn ensure_state(state: &StatePaths) -> Result<()> {
    state.ensure_layout()?;
    ContynuConfig::ensure_exists(&state.config_path())?;
    let _ = MetadataStore::open(state.sqlite_db())?;
    let _ = BlobStore::new(state.blobs_root());
    Ok(())
}

fn start_project(state: &StatePaths, cwd: &PathBuf) -> Result<()> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    if let Some(project_id) = store.primary_project_id()? {
        println!("Primary project is already active: {project_id}");
        return Ok(());
    }

    let project_id = ProjectId::new();
    store.register_session(&contynu_core::SessionRecord {
        session_id: project_id.clone(),
        project_id: Some(project_id.to_string()),
        status: "active".into(),
        cli_name: Some("manual".into()),
        cli_version: None,
        model_name: None,
        cwd: Some(cwd.display().to_string()),
        repo_root: Some(cwd.display().to_string()),
        host_fingerprint: None,
        started_at: chrono::Utc::now(),
        ended_at: None,
    })?;
    store.set_primary_project_id(&project_id)?;
    let journal = Journal::open(state.journal_path_for_project(&project_id))?;
    let (event, append) = journal.append(EventDraft::new(
        project_id.clone(),
        None,
        contynu_core::Actor::System,
        EventType::SessionStarted,
        serde_json::json!({
            "cwd": cwd.display().to_string(),
            "adapter_kind": "manual",
            "continued": false,
        }),
    ))?;
    store.record_event(&event, &journal.path().display().to_string(), append)?;
    println!("Started a new primary project: {project_id}");
    Ok(())
}

fn run(state: &StatePaths, cwd: &PathBuf, command: RunCommand) -> Result<()> {
    let outcome = RuntimeEngine::run(RunConfig {
        state_dir: state.root().to_path_buf(),
        cwd: cwd.clone(),
        command: command.command.into_iter().map(Into::into).collect(),
        ignore_patterns: command.ignore_patterns,
        checkpoint_on_exit: !command.no_checkpoint,
        project_id: command.project.map(ProjectId::parse).transpose()?,
    })?;
    print_run_footer(&outcome);
    Ok(())
}

fn launch_llm(
    state: &StatePaths,
    cwd: &PathBuf,
    executable: &str,
    command: LlmCommand,
) -> Result<()> {
    // Auto-discover and import existing LLM sessions (non-fatal)
    if let Err(e) = auto_import_sessions(state) {
        eprintln!("Note: session auto-import: {e}");
    }

    // Resolve project ID for MCP registration
    let project_id_for_mcp = {
        let store = MetadataStore::open(state.sqlite_db())?;
        command
            .project
            .as_ref()
            .map(|p| ProjectId::parse(p.clone()))
            .transpose()?
            .or(store.primary_project_id()?)
    };

    // Auto-register MCP server for this CLI (non-fatal on failure)
    if let Some(ref pid) = project_id_for_mcp {
        if let Err(e) =
            mcp_registration::ensure_mcp_registered(executable, state.root(), cwd, pid.as_str())
        {
            eprintln!("Warning: MCP auto-registration: {e}");
        }
    }

    let mut argv = vec![executable.to_string()];
    argv.extend(command.args);
    let outcome = RuntimeEngine::run(RunConfig {
        state_dir: state.root().to_path_buf(),
        cwd: cwd.clone(),
        command: argv.into_iter().map(Into::into).collect(),
        ignore_patterns: command.ignore_patterns,
        checkpoint_on_exit: !command.no_checkpoint,
        project_id: command.project.map(ProjectId::parse).transpose()?,
    })?;
    print_run_footer(&outcome);
    Ok(())
}

fn passthrough(state: &StatePaths, cwd: &PathBuf, command: Vec<OsString>) -> Result<()> {
    let outcome = RuntimeEngine::run(RunConfig {
        state_dir: state.root().to_path_buf(),
        cwd: cwd.clone(),
        command,
        ignore_patterns: Vec::new(),
        checkpoint_on_exit: true,
        project_id: None,
    })?;
    print_run_footer(&outcome);
    Ok(())
}

fn checkpoint(state: &StatePaths, project: Option<&str>, reason: &str) -> Result<()> {
    ensure_state(state)?;
    let project_id = resolve_project_id(state, project)?;
    let journal = Journal::open(state.journal_path_for_project(&project_id))?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let blobs = BlobStore::new(state.blobs_root());
    let manager = CheckpointManager::new(state, &store, &blobs);
    let (manifest, packet) = manager.create_checkpoint(&journal, &project_id, reason, None)?;
    print_checkpoint_result(&manifest, &packet)
}

fn resume(state: &StatePaths, project: Option<&str>, target_model: Option<String>) -> Result<()> {
    ensure_state(state)?;
    let project_id = resolve_project_id(state, project)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let blobs = BlobStore::new(state.blobs_root());
    let manager = CheckpointManager::new(state, &store, &blobs);
    let packet = manager.build_packet(&project_id, target_model)?;
    print_rehydration_packet(
        if packet.target_model.is_some() {
            "Handoff Ready"
        } else {
            "Resume Ready"
        },
        &packet,
    )
}

fn replay(state: &StatePaths, project: Option<&str>) -> Result<()> {
    let project_id = resolve_project_id(state, project)?;
    let journal = Journal::open(state.journal_path_for_project(&project_id))?;
    let replay = journal.replay()?;
    println!("Replay for project {project_id}");
    println!();
    if replay.is_empty() {
        println!("No canonical events have been recorded yet.");
        return Ok(());
    }
    for item in replay {
        println!(
            "#{:04}  {}  {} / {}{}",
            item.event.seq,
            item.event.ts,
            item.event.actor.as_str(),
            item.event.event_type.as_str(),
            format_turn_suffix(item.event.turn_id.as_ref())
        );
    }
    Ok(())
}

fn inspect(state: &StatePaths, command: InspectCommand) -> Result<()> {
    let store = MetadataStore::open(state.sqlite_db())?;
    match command {
        InspectCommand::Project { id } => {
            let project_id = match id {
                Some(id) => ProjectId::parse(id)?,
                None => resolve_primary_project(&store)?,
            };
            let session = store
                .get_session(&project_id)?
                .ok_or_else(|| anyhow!("project not found"))?;
            let turns = store.list_turns_for_session(&project_id)?;
            let events = store.list_events_for_session(&project_id)?;
            println!("Project {}", project_id);
            println!();
            print_kv("Status", &session.status);
            print_optional_kv("Launcher", session.cli_name.as_deref());
            print_optional_kv("Working directory", session.cwd.as_deref());
            print_kv("Started", &session.started_at.to_rfc3339());
            print_kv("Turns", &turns.len().to_string());
            print_kv("Events", &events.len().to_string());
            if let Some(turn) = turns.first() {
                print_kv("Latest turn", turn.turn_id.as_str());
                print_kv("Latest turn status", &turn.status);
            }
            println!();
            println!("Recent events");
            println!();
            for event in events
                .iter()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                println!(
                    "- #{:04} {} {} / {}{}",
                    event.seq,
                    event.ts,
                    event.actor,
                    event.event_type,
                    format_turn_suffix(event.turn_id.as_ref())
                );
            }
            Ok(())
        }
        InspectCommand::Event { id } => {
            let event_id = EventId::parse(id)?;
            let event = store
                .get_event(&event_id)?
                .ok_or_else(|| anyhow!("event not found"))?;
            println!("Event {}", event.event_id);
            println!();
            print_kv("Project", event.session_id.as_str());
            print_optional_kv("Turn", event.turn_id.as_ref().map(|value| value.as_str()));
            print_kv("Sequence", &event.seq.to_string());
            print_kv("Time", &event.ts.to_rfc3339());
            print_kv("Actor", &event.actor);
            print_kv("Type", &event.event_type);
            print_kv("Checksum", &event.checksum);
            println!();
            println!("Payload");
            println!();
            print_pretty_json_value(&event.payload_json)?;
            Ok(())
        }
    }
}

fn search(state: &StatePaths, command: SearchCommand) -> Result<()> {
    let store = MetadataStore::open(state.sqlite_db())?;
    match command {
        SearchCommand::Exact { query } => {
            let results = store.search_exact(&query)?;
            println!("Exact search for \"{}\"", query);
            println!();
            if results.is_empty() {
                println!("No exact matches were found.");
                return Ok(());
            }
            for event in results {
                println!(
                    "- #{:04} {} / {}{}",
                    event.seq,
                    event.actor,
                    event.event_type,
                    format_turn_suffix(event.turn_id.as_ref())
                );
                println!("  {}", summarize_json(&event.payload_json));
            }
            Ok(())
        }
        SearchCommand::Memory { query } => {
            let results = store.search_memory(&query)?;
            println!("Memory search for \"{}\"", query);
            println!();
            if results.is_empty() {
                println!("No memory objects matched that search.");
                return Ok(());
            }
            for memory in results {
                println!(
                    "- {} [{}] {}",
                    memory.kind.as_str(),
                    memory.status,
                    memory.text
                );
            }
            Ok(())
        }
    }
}

fn artifacts(state: &StatePaths, command: ArtifactsCommand) -> Result<()> {
    let store = MetadataStore::open(state.sqlite_db())?;
    match command {
        ArtifactsCommand::List { project } => {
            let project = project.map(ProjectId::parse).transpose()?;
            let artifacts = store.list_artifacts(project.as_ref())?;
            println!("Artifacts");
            println!();
            if artifacts.is_empty() {
                println!("No artifacts have been recorded yet.");
                return Ok(());
            }
            for artifact in artifacts {
                println!(
                    "- {}  {} bytes  {}",
                    artifact.kind, artifact.size_bytes, artifact.sha256
                );
            }
            Ok(())
        }
    }
}

fn status(state: &StatePaths, project: Option<&str>) -> Result<()> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let project_id = resolve_project_id(state, project)?;
    let session = store
        .get_session(&project_id)?
        .ok_or_else(|| anyhow!("project not found"))?;
    let turns = store.list_turns_for_session(&project_id)?;
    let artifacts = store.list_artifacts(Some(&project_id))?;
    let files = store.list_current_files(&project_id)?;
    let memory = store.list_memory_objects(&project_id, None)?;
    let events = store.list_events_for_session(&project_id)?;
    let latest_turn = turns.first().cloned();
    let recent_events = events
        .iter()
        .rev()
        .take(5)
        .map(|event| {
            serde_json::json!({
                "seq": event.seq,
                "event_type": event.event_type,
                "ts": event.ts,
            })
        })
        .collect::<Vec<_>>();
    println!("Project status");
    println!();
    print_kv("Project", project_id.as_str());
    print_kv("Status", &session.status);
    print_optional_kv("Launcher", session.cli_name.as_deref());
    print_optional_kv("Working directory", session.cwd.as_deref());
    print_kv("Started", &session.started_at.to_rfc3339());
    if let Some(turn) = latest_turn {
        print_kv("Latest turn", turn.turn_id.as_str());
        print_kv("Latest turn status", &turn.status);
        print_kv("Latest turn started", &turn.started_at.to_rfc3339());
    }
    println!();
    println!("Counts");
    println!();
    print_kv("Turns", &turns.len().to_string());
    print_kv("Events", &events.len().to_string());
    print_kv("Artifacts", &artifacts.len().to_string());
    print_kv("Tracked files", &files.len().to_string());
    print_kv("Memory objects", &memory.len().to_string());
    println!();
    println!("Recent events");
    println!();
    for event in recent_events {
        println!(
            "- #{}  {}  {}",
            event["seq"].as_u64().unwrap_or_default(),
            event["ts"].as_str().unwrap_or(""),
            event["event_type"].as_str().unwrap_or("")
        );
    }
    Ok(())
}

fn projects(state: &StatePaths) -> Result<()> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let primary = store.primary_project_id()?;
    let sessions = store.list_sessions()?;
    println!("Projects");
    println!();
    if sessions.is_empty() {
        println!("No projects have been started yet.");
        return Ok(());
    }
    for session in sessions {
        let marker = if primary.as_ref() == Some(&session.session_id) {
            "primary"
        } else {
            "project"
        };
        println!("- {}  {}", session.session_id, marker);
        println!("  status: {}", session.status);
        if let Some(cli_name) = session.cli_name.as_deref() {
            println!("  launcher: {}", cli_name);
        }
        if let Some(cwd) = session.cwd.as_deref() {
            println!("  cwd: {}", cwd);
        }
        println!("  started: {}", session.started_at.to_rfc3339());
    }
    Ok(())
}

fn recent(state: &StatePaths, limit: usize) -> Result<()> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let sessions = store.list_sessions()?;
    println!("Recent activity");
    println!();
    let mut any = false;
    for session in sessions.into_iter().take(limit) {
        let turns = store.list_turns_for_session(&session.session_id)?;
        let latest_turn = turns.first().cloned();
        any = true;
        println!("- {}", session.session_id);
        println!("  status: {}", session.status);
        if let Some(cli_name) = session.cli_name.as_deref() {
            println!("  launcher: {}", cli_name);
        }
        if let Some(cwd) = session.cwd.as_deref() {
            println!("  cwd: {}", cwd);
        }
        println!("  started: {}", session.started_at.to_rfc3339());
        if let Some(turn) = latest_turn {
            println!(
                "  latest turn: {} ({})",
                turn.turn_id,
                turn.started_at.to_rfc3339()
            );
        } else {
            println!("  latest turn: none yet");
        }
    }
    if !any {
        println!("No recent activity yet.");
    }
    Ok(())
}

fn config_command(state: &StatePaths, command: ConfigCommand) -> Result<()> {
    ensure_state(state)?;
    match command {
        ConfigCommand::Validate => {
            let config = ContynuConfig::load(&state.config_path())?;
            println!("Config is valid.");
            println!();
            print_kv("Path", &state.config_path().display().to_string());
            print_kv("Launchers", &config.llm_launchers.len().to_string());
            println!();
            for launcher in config.llm_launchers {
                println!("- {}", launcher.command);
                if !launcher.aliases.is_empty() {
                    println!("  aliases: {}", launcher.aliases.join(", "));
                }
                println!("  hydrate: {}", yes_no(launcher.hydrate));
                println!("  use pty: {}", yes_no(launcher.use_pty));
                println!(
                    "  delivery: {}",
                    hydration_delivery_name(launcher.hydration_delivery)
                );
            }
            Ok(())
        }
        ConfigCommand::Show => {
            let raw = std::fs::read_to_string(state.config_path())?;
            println!("Config file: {}", state.config_path().display());
            println!();
            println!("{raw}");
            Ok(())
        }
    }
}

fn doctor(state: &StatePaths) -> Result<()> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let primary_project = store.primary_project_id()?;
    let sessions = store.list_sessions()?;
    let config = ContynuConfig::load(&state.config_path())?;
    println!("Contynu doctor");
    println!();
    print_kv("State root", &state.root().display().to_string());
    print_kv("Config", &state.config_path().display().to_string());
    print_kv("SQLite", &state.sqlite_db().display().to_string());
    print_kv("Journal root", &state.journal_root().display().to_string());
    print_kv("Runtime root", &state.runtime_root().display().to_string());
    print_kv(
        "Checkpoints",
        &state.checkpoints_root().display().to_string(),
    );
    print_optional_kv(
        "Primary project",
        primary_project.as_ref().map(|value| value.as_str()),
    );
    print_kv("Projects", &sessions.len().to_string());
    print_kv("Artifacts", &store.list_artifacts(None)?.len().to_string());
    print_kv(
        "Configured launchers",
        &config.llm_launchers.len().to_string(),
    );
    Ok(())
}

fn repair(state: &StatePaths, project: Option<&str>) -> Result<()> {
    ensure_state(state)?;
    match project {
        Some(project) => {
            let project_id = ProjectId::parse(project.to_string())?;
            let journal = Journal::open(state.journal_path_for_project(&project_id))?;
            let repair = journal.repair_truncated_tail()?;
            let store = MetadataStore::open(state.sqlite_db())?;
            store.reconcile_session(&journal, &project_id)?;
            print_repair_result(&project_id, repair)
        }
        None => {
            let project_id = resolve_project_id(state, None)?;
            let journal = Journal::open(state.journal_path_for_project(&project_id))?;
            let repair = journal.repair_truncated_tail()?;
            let store = MetadataStore::open(state.sqlite_db())?;
            store.reconcile_session(&journal, &project_id)?;
            print_repair_result(&project_id, repair)
        }
    }
}

fn resolve_project_id(state: &StatePaths, explicit: Option<&str>) -> Result<ProjectId> {
    if let Some(id) = explicit {
        return Ok(ProjectId::parse(id.to_string())?);
    }
    let store = MetadataStore::open(state.sqlite_db())?;
    resolve_primary_project(&store)
}

fn resolve_primary_project(store: &MetadataStore) -> Result<ProjectId> {
    store.primary_project_id()?.ok_or_else(|| {
        anyhow!(
            "no primary project found; run `contynu start-project` or `contynu run -- ...` first"
        )
    })
}

fn print_run_footer(outcome: &RunOutcome) {
    let lines = if outcome.interrupted {
        vec![
            "Contynu paused here.".to_string(),
            format!(
                "Saved turn {} in project {}.",
                short_id(outcome.turn_id.as_str()),
                short_id(outcome.project_id.as_str())
            ),
        ]
    } else {
        vec![
            "Let's contynu another time. Goodbye for now.".to_string(),
            format!(
                "Saved turn {} in project {}.",
                short_id(outcome.turn_id.as_str()),
                short_id(outcome.project_id.as_str())
            ),
        ]
    };

    let width = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    eprintln!();
    eprintln!("┌{}┐", "─".repeat(width + 2));
    for line in lines {
        let padding = width.saturating_sub(line.chars().count());
        eprintln!("│ {}{} │", line, " ".repeat(padding));
    }
    eprintln!("└{}┘", "─".repeat(width + 2));
    eprintln!();
}

fn short_id(value: &str) -> &str {
    let keep = 12;
    if value.len() <= keep {
        value
    } else {
        &value[..keep]
    }
}

fn print_checkpoint_result(
    manifest: &contynu_core::CheckpointManifest,
    packet: &contynu_core::RehydrationPacket,
) -> Result<()> {
    println!("Checkpoint created");
    println!();
    print_kv("Project", manifest.project_id.as_str());
    print_kv("Checkpoint", manifest.checkpoint_id.as_str());
    print_kv("Reason", &manifest.reason);
    print_kv("Created", &manifest.created_at.to_rfc3339());
    print_kv("Last sequence", &manifest.last_seq.to_string());
    print_optional_kv("Rehydration blob", manifest.rehydration_blob_sha.as_deref());
    print_kv("Checkpoint directory", &manifest.checkpoint_dir);
    println!();
    print_rehydration_packet("Rehydration packet", packet)
}

fn print_rehydration_packet(title: &str, packet: &contynu_core::RehydrationPacket) -> Result<()> {
    println!("{}", title);
    println!();
    print_kv("Project", packet.project_id.as_str());
    print_optional_kv("Target model", packet.target_model.as_deref());
    print_kv("Schema", &packet.schema_version.to_string());
    println!();
    print_section("Mission", &[packet.mission.clone()]);
    print_section("Current state", &[packet.current_state.clone()]);
    print_section("Stable facts", &packet.stable_facts);
    print_section("Constraints", &packet.constraints);
    print_section("Decisions", &packet.decisions);
    print_section("Open loops", &packet.open_loops);
    print_section("Relevant files", &packet.relevant_files);
    let artifacts = packet
        .relevant_artifacts
        .iter()
        .map(|artifact| format!("{}  {}  {}", artifact.kind, artifact.path, artifact.sha256))
        .collect::<Vec<_>>();
    print_section("Relevant artifacts", &artifacts);
    print_section("Recent context", &packet.recent_verbatim_context);
    print_section("Retrieval guidance", &packet.retrieval_guidance);
    Ok(())
}

fn openclaw_command(
    state: &StatePaths,
    cwd: &std::path::PathBuf,
    command: OpenClawCommand,
) -> Result<()> {
    match command {
        OpenClawCommand::Setup { openclaw_config } => openclaw_setup(state, cwd, openclaw_config),
        OpenClawCommand::Status => openclaw_status(state),
    }
}

fn openclaw_setup(
    state: &StatePaths,
    _cwd: &std::path::PathBuf,
    openclaw_config: Option<std::path::PathBuf>,
) -> Result<()> {
    // Initialize Contynu state
    state.ensure_layout()?;
    contynu_core::ContynuConfig::ensure_exists(&state.root().join("config.json"))?;

    // Ensure primary project exists
    let store = MetadataStore::open(state.sqlite_db())?;
    let project_id = match store.primary_project_id()? {
        Some(id) => id,
        None => {
            let id = contynu_core::ProjectId::new();
            store.register_session(&contynu_core::SessionRecord {
                session_id: id.clone(),
                project_id: None,
                status: "active".into(),
                cli_name: Some("openclaw".into()),
                cli_version: None,
                model_name: None,
                cwd: None,
                repo_root: None,
                host_fingerprint: None,
                started_at: chrono::Utc::now(),
                ended_at: None,
            })?;
            store.set_primary_project_id(&id)?;
            id
        }
    };

    // Register MCP server in OpenClaw config
    let oc_config = openclaw_config.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(home)
            .join(".openclaw")
            .join("openclaw.json")
    });
    if let Err(e) =
        mcp_registration::ensure_mcp_registered("openclaw", state.root(), &oc_config, project_id.as_str())
    {
        eprintln!("Warning: Could not register MCP server in OpenClaw config: {e}");
    }

    // Create agent mapping file
    let mapping_path = state.root().join("openclaw-agents.json");
    if !mapping_path.exists() {
        std::fs::write(&mapping_path, "{}")?;
    }

    println!("Contynu + OpenClaw integration ready.\n");
    println!("  State directory:  {}", state.root().display());
    println!("  Primary project:  {}", project_id);
    println!("  Agent mapping:    {}", mapping_path.display());
    println!();
    println!("Install the plugin:");
    println!("  npm install -g contynu-openclaw");
    println!();
    println!("Add to your OpenClaw config ({}):", oc_config.display());
    println!("  {{");
    println!("    \"plugins\": {{");
    println!("      \"contynu-openclaw\": {{ \"enabled\": true }}");
    println!("    }}");
    println!("  }}");
    println!();
    println!("Then restart OpenClaw. Every agent gets permanent memory automatically.");

    Ok(())
}

fn openclaw_status(state: &StatePaths) -> Result<()> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;

    println!("Contynu + OpenClaw integration status\n");

    // State dir
    println!("  State directory:  {}", state.root().display());

    // Primary project
    match store.primary_project_id()? {
        Some(id) => println!("  Primary project:  {}", id),
        None => println!("  Primary project:  (none)"),
    }

    // Agent mapping
    let mapping_path = state.root().join("openclaw-agents.json");
    if mapping_path.exists() {
        let content = std::fs::read_to_string(&mapping_path)?;
        let map: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        let count = map.as_object().map_or(0, |m| m.len());
        println!("  Mapped agents:    {}", count);
    } else {
        println!("  Agent mapping:    (not created)");
    }

    // Memory counts
    if let Some(id) = store.primary_project_id()? {
        let total = store.count_active_memories(&id, None)?;
        println!("  Active memories:  {}", total);
    }

    // MCP server check
    let oc_config_path = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".openclaw").join("openclaw.json"))
        .unwrap_or_default();
    if oc_config_path.exists() {
        let content = std::fs::read_to_string(&oc_config_path).unwrap_or_default();
        if content.contains("contynu") {
            println!("  MCP server:       registered");
        } else {
            println!("  MCP server:       not registered (run `contynu openclaw setup`)");
        }
    } else {
        println!("  OpenClaw config:  not found");
    }

    Ok(())
}

/// Auto-discover and import existing LLM session files that haven't been imported yet.
fn auto_import_sessions(state: &StatePaths) -> Result<()> {
    let tracking_path = state.root().join("imported-sessions.json");
    let mut imported: std::collections::HashSet<String> = if tracking_path.exists() {
        let content = std::fs::read_to_string(&tracking_path)?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        std::collections::HashSet::new()
    };

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let home = std::path::Path::new(&home);
    let mut new_files: Vec<(std::path::PathBuf, &str)> = Vec::new();

    // Codex rollout files
    let codex_dir = home.join(".codex").join("sessions");
    if codex_dir.exists() {
        if let Ok(walker) = glob_files(&codex_dir, "rollout-*.jsonl") {
            for path in walker {
                let key = path.display().to_string();
                if !imported.contains(&key) {
                    new_files.push((path, "codex-jsonl"));
                }
            }
        }
    }

    // Gemini session files
    let gemini_dir = home.join(".gemini").join("tmp");
    if gemini_dir.exists() {
        if let Ok(walker) = glob_files(&gemini_dir, "session-*.json") {
            for path in walker {
                let key = path.display().to_string();
                if !imported.contains(&key) {
                    new_files.push((path, "gemini"));
                }
            }
        }
    }

    if new_files.is_empty() {
        return Ok(());
    }

    // Import new files
    let store = MetadataStore::open(state.sqlite_db())?;
    let project_id = match store.primary_project_id()? {
        Some(id) => id,
        None => return Ok(()), // no project yet
    };
    let journal = contynu_core::Journal::open(state.journal_path_for_project(&project_id))?;

    let mut total_imported = 0usize;
    for (path, format) in &new_files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if content.len() < 50 { continue; } // skip tiny/empty files

        let turn_id = contynu_core::TurnId::new();
        store.register_turn(&contynu_core::TurnRecord {
            turn_id: turn_id.clone(),
            session_id: project_id.clone(),
            status: "started".into(),
            started_at: chrono::Utc::now(),
            completed_at: None,
            summary_memory_id: None,
        })?;

        let mut event_count = 0usize;
        match *format {
            "gemini" => {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(messages) = data.get("messages").and_then(|m| m.as_array()) {
                        for msg in messages {
                            let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            let text = msg.get("content").and_then(|c| c.as_array())
                                .map(|parts| parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"))
                                .unwrap_or_default();
                            if text.is_empty() || text.len() < 5 { continue; }
                            let (et, actor) = match msg_type {
                                "user" => ("message_input", "user"),
                                "model" => ("message_output", "assistant"),
                                _ => continue,
                            };
                            if let Ok(il) = serde_json::from_value::<contynu_core::event::IngestLine>(serde_json::json!({"event_type": et, "actor": actor, "payload": {"content": [{"type": "text", "text": text}]}})) {
                                let draft = il.into_draft(project_id.clone(), Some(turn_id.clone()));
                                if let Ok((event, append)) = journal.append(draft) {
                                    let _ = store.record_event(&event, &journal.path().display().to_string(), append);
                                    event_count += 1;
                                }
                            }
                        }
                    }
                }
            }
            "codex-jsonl" => {
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() { continue; }
                    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(line) {
                        if obj.get("type").and_then(|t| t.as_str()) != Some("response_item") { continue; }
                        if let Some(payload) = obj.get("payload") {
                            let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                            let text = payload.get("content").and_then(|c| c.as_array())
                                .map(|parts| parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"))
                                .unwrap_or_default();
                            if text.is_empty() || text.len() < 5 { continue; }
                            let (et, actor) = match role {
                                "user" => ("message_input", "user"),
                                "assistant" => ("message_output", "assistant"),
                                _ => continue,
                            };
                            if let Ok(il) = serde_json::from_value::<contynu_core::event::IngestLine>(serde_json::json!({"event_type": et, "actor": actor, "payload": {"content": [{"type": "text", "text": text}]}})) {
                                let draft = il.into_draft(project_id.clone(), Some(turn_id.clone()));
                                if let Ok((event, append)) = journal.append(draft) {
                                    let _ = store.record_event(&event, &journal.path().display().to_string(), append);
                                    event_count += 1;
                                }
                            }
                        }
                    }
                }
            }
            _ => continue,
        }

        if event_count > 0 {
            let _ = contynu_core::derive_memory_from_ingested_events(
                &journal, &store, &project_id, &turn_id,
                Some(format.to_string()), None,
            );
            total_imported += 1;
        }
        store.update_turn_status(&turn_id, "completed", Some(chrono::Utc::now()))?;
        imported.insert(path.display().to_string());
    }

    // Save tracking file
    std::fs::write(&tracking_path, serde_json::to_string_pretty(&imported)?)?;

    if total_imported > 0 {
        eprintln!(
            "Auto-imported {} session file(s) from Codex/Gemini history",
            total_imported
        );
    }
    Ok(())
}

/// Walk a directory recursively and find files matching a pattern.
fn glob_files(dir: &std::path::Path, pattern: &str) -> Result<Vec<std::path::PathBuf>> {
    let mut results = Vec::new();
    for entry in walkdir::WalkDir::new(dir).max_depth(6).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            if let Some(name) = entry.file_name().to_str() {
                if matches_simple_glob(name, pattern) {
                    results.push(entry.into_path());
                }
            }
        }
    }
    Ok(results)
}

fn matches_simple_glob(name: &str, pattern: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        name.ends_with(suffix)
    } else if let Some((prefix, suffix)) = pattern.split_once('*') {
        name.starts_with(prefix) && name.ends_with(suffix)
    } else {
        name == pattern
    }
}

fn import_conversations(
    state: &StatePaths,
    project: Option<&str>,
    files: &[std::path::PathBuf],
    format: &str,
    adapter: Option<&str>,
) -> Result<()> {
    ensure_state(state)?;
    let project_id = resolve_project_id(state, project)?;
    let journal = contynu_core::Journal::open(state.journal_path_for_project(&project_id))?;
    let store = MetadataStore::open(state.sqlite_db())?;

    let mut total_events = 0usize;
    let mut total_memories = 0usize;

    for file_path in files {
        let content = std::fs::read_to_string(file_path)
            .map_err(|e| anyhow!("Failed to read {}: {e}", file_path.display()))?;

        let detected_format = if format == "auto" {
            if file_path.extension().map_or(false, |ext| ext == "jsonl") {
                // Check if it's Codex rollout (has "type":"session_meta") or Claude JSONL
                if content.contains("session_meta") || content.contains("response_item") {
                    "codex-jsonl"
                } else {
                    "claude-jsonl"
                }
            } else if content.contains("\"sessionId\"") && content.contains("\"messages\"") {
                "gemini"
            } else if content.contains("\"mapping\"") && content.contains("\"message\"") {
                "chatgpt"
            } else if content.trim_start().starts_with('[') || content.trim_start().starts_with('{') {
                "chatgpt"
            } else {
                "text"
            }
        } else {
            format
        };

        let turn_id = contynu_core::TurnId::new();
        store.register_turn(&contynu_core::TurnRecord {
            turn_id: turn_id.clone(),
            session_id: project_id.clone(),
            status: "started".into(),
            started_at: chrono::Utc::now(),
            completed_at: None,
            summary_memory_id: None,
        })?;

        let adapter_name = adapter.unwrap_or(detected_format);
        let mut event_count = 0usize;

        match detected_format {
            "claude-jsonl" => {
                // Claude Code JSONL: one JSON object per line with role + content
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() { continue; }
                    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(line) {
                        let role = obj.get("role").and_then(|v| v.as_str()).unwrap_or("system");
                        let text = obj.get("content").and_then(|v| v.as_str())
                            .or_else(|| obj.get("text").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        if text.is_empty() { continue; }
                        let (event_type, actor) = match role {
                            "user" | "human" => ("message_input", "user"),
                            "assistant" => ("message_output", "assistant"),
                            _ => ("message_output", "system"),
                        };
                        let ingest_line = contynu_core::IngestLine {
                            event_type: serde_json::from_value(serde_json::json!(event_type))?,
                            actor: serde_json::from_value(serde_json::json!(actor))?,
                            payload: serde_json::json!({"content": [{"type": "text", "text": text}]}),
                            ts: None,
                        };
                        let draft = ingest_line.into_draft(project_id.clone(), Some(turn_id.clone()));
                        let (event, append) = journal.append(draft)?;
                        store.record_event(&event, &journal.path().display().to_string(), append)?;
                        event_count += 1;
                    }
                }
            }
            "chatgpt" => {
                // ChatGPT conversations.json export
                let data: serde_json::Value = serde_json::from_str(&content)
                    .map_err(|e| anyhow!("Failed to parse ChatGPT JSON: {e}"))?;
                let conversations = if data.is_array() { data.as_array().cloned().unwrap_or_default() } else { vec![data] };
                for convo in conversations {
                    let mapping = convo.get("mapping").and_then(|m| m.as_object());
                    if let Some(mapping) = mapping {
                        for (_id, node) in mapping {
                            let msg = node.get("message");
                            if let Some(msg) = msg {
                                let role = msg.get("author").and_then(|a| a.get("role")).and_then(|r| r.as_str()).unwrap_or("system");
                                let parts = msg.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array());
                                let text = parts.map(|ps| ps.iter().filter_map(|p| p.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                                if text.is_empty() { continue; }
                                let (event_type, actor) = match role {
                                    "user" => ("message_input", "user"),
                                    "assistant" => ("message_output", "assistant"),
                                    _ => continue,
                                };
                                let ingest_line = contynu_core::IngestLine {
                                    event_type: serde_json::from_value(serde_json::json!(event_type))?,
                                    actor: serde_json::from_value(serde_json::json!(actor))?,
                                    payload: serde_json::json!({"content": [{"type": "text", "text": text}]}),
                                    ts: msg.get("create_time").and_then(|t| t.as_f64()).map(|ts| {
                                        chrono::DateTime::from_timestamp(ts as i64, 0).map(|dt| dt.to_utc())
                                    }).flatten(),
                                };
                                let draft = ingest_line.into_draft(project_id.clone(), Some(turn_id.clone()));
                                let (event, append) = journal.append(draft)?;
                                store.record_event(&event, &journal.path().display().to_string(), append)?;
                                event_count += 1;
                            }
                        }
                    }
                }
            }
            "gemini" => {
                // Gemini CLI session JSON: {"sessionId", "messages": [{"type": "user"|"model", "content": [{"text"}]}]}
                let data: serde_json::Value = serde_json::from_str(&content)
                    .map_err(|e| anyhow!("Failed to parse Gemini JSON: {e}"))?;
                let messages = data.get("messages").and_then(|m| m.as_array());
                if let Some(messages) = messages {
                    for msg in messages {
                        let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("system");
                        let text = msg.get("content")
                            .and_then(|c| c.as_array())
                            .map(|parts| parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n"))
                            .unwrap_or_default();
                        if text.is_empty() { continue; }
                        let (event_type, actor) = match msg_type {
                            "user" => ("message_input", "user"),
                            "model" => ("message_output", "assistant"),
                            _ => continue,
                        };
                        let ts = msg.get("timestamp").and_then(|t| t.as_str())
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|dt| dt.with_timezone(&chrono::Utc));
                        let ingest_line = contynu_core::IngestLine {
                            event_type: serde_json::from_value(serde_json::json!(event_type))?,
                            actor: serde_json::from_value(serde_json::json!(actor))?,
                            payload: serde_json::json!({"content": [{"type": "text", "text": text}]}),
                            ts,
                        };
                        let draft = ingest_line.into_draft(project_id.clone(), Some(turn_id.clone()));
                        let (event, append) = journal.append(draft)?;
                        store.record_event(&event, &journal.path().display().to_string(), append)?;
                        event_count += 1;
                    }
                }
            }
            "codex-jsonl" => {
                // Codex CLI rollout JSONL: {"type":"response_item","payload":{"type":"message","role":"user"|"assistant","content":[{"text"}]}}
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() { continue; }
                    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(line) {
                        let item_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        if item_type != "response_item" { continue; }
                        let payload = match obj.get("payload") { Some(p) => p, None => continue };
                        let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                        let text = payload.get("content")
                            .and_then(|c| c.as_array())
                            .map(|parts| parts.iter()
                                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>().join("\n"))
                            .unwrap_or_default();
                        if text.is_empty() || text.len() < 5 { continue; }
                        let (event_type, actor) = match role {
                            "user" => ("message_input", "user"),
                            "assistant" => ("message_output", "assistant"),
                            _ => continue,
                        };
                        let ingest_line = contynu_core::IngestLine {
                            event_type: serde_json::from_value(serde_json::json!(event_type))?,
                            actor: serde_json::from_value(serde_json::json!(actor))?,
                            payload: serde_json::json!({"content": [{"type": "text", "text": text}]}),
                            ts: obj.get("timestamp").and_then(|t| t.as_str())
                                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                .map(|dt| dt.with_timezone(&chrono::Utc)),
                        };
                        let draft = ingest_line.into_draft(project_id.clone(), Some(turn_id.clone()));
                        let (event, append) = journal.append(draft)?;
                        store.record_event(&event, &journal.path().display().to_string(), append)?;
                        event_count += 1;
                    }
                }
            }
            _ => {
                // Plain text: treat entire file as one message
                let ingest_line = contynu_core::IngestLine {
                    event_type: serde_json::from_value(serde_json::json!("message_output"))?,
                    actor: serde_json::from_value(serde_json::json!("assistant"))?,
                    payload: serde_json::json!({"content": [{"type": "text", "text": content}]}),
                    ts: None,
                };
                let draft = ingest_line.into_draft(project_id.clone(), Some(turn_id.clone()));
                let (event, append) = journal.append(draft)?;
                store.record_event(&event, &journal.path().display().to_string(), append)?;
                event_count += 1;
            }
        }

        let memory_count = if event_count > 0 {
            contynu_core::derive_memory_from_ingested_events(
                &journal, &store, &project_id, &turn_id,
                Some(adapter_name.to_string()), None,
            )?
        } else {
            0
        };

        store.update_turn_status(&turn_id, "completed", Some(chrono::Utc::now()))?;
        total_events += event_count;
        total_memories += memory_count;
        eprintln!(
            "Imported {} ({} format): {} events, {} memories",
            file_path.display(), detected_format, event_count, memory_count
        );
    }

    eprintln!(
        "Total: {} events, {} memories imported into project {}",
        total_events, total_memories, project_id
    );
    Ok(())
}

fn ingest(
    state: &StatePaths,
    project: Option<&str>,
    adapter: Option<&str>,
    model: Option<&str>,
    derive_memory: bool,
) -> Result<()> {
    ensure_state(state)?;
    let project_id = resolve_project_id(state, project)?;
    let journal = contynu_core::Journal::open(state.journal_path_for_project(&project_id))?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let turn_id = contynu_core::TurnId::new();

    // Register turn
    store.register_turn(&contynu_core::TurnRecord {
        turn_id: turn_id.clone(),
        session_id: project_id.clone(),
        status: "started".into(),
        started_at: chrono::Utc::now(),
        completed_at: None,
        summary_memory_id: None,
    })?;

    let stdin = std::io::stdin().lock();
    let mut event_count = 0usize;

    for line in std::io::BufRead::lines(stdin) {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let ingest_line: contynu_core::IngestLine = serde_json::from_str(&line)
            .map_err(|e| anyhow!("Failed to parse ingest line: {e}\nLine: {line}"))?;
        let draft = ingest_line.into_draft(project_id.clone(), Some(turn_id.clone()));
        let (event, append) = journal.append(draft)?;
        store.record_event(&event, &journal.path().display().to_string(), append)?;
        event_count += 1;
    }

    let memory_count = if derive_memory && event_count > 0 {
        contynu_core::derive_memory_from_ingested_events(
            &journal,
            &store,
            &project_id,
            &turn_id,
            adapter.map(String::from),
            model.map(String::from),
        )?
    } else {
        0
    };

    store.update_turn_status(&turn_id, "completed", Some(chrono::Utc::now()))?;
    eprintln!(
        "Ingested {} events, derived {} memory objects for project {}",
        event_count, memory_count, project_id
    );
    Ok(())
}

fn export_memory(
    state: &StatePaths,
    project: Option<&str>,
    max_chars: usize,
    with_markers: bool,
) -> Result<()> {
    ensure_state(state)?;
    let project_id = resolve_project_id(state, project)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let memories = store.list_active_memory_objects(&project_id, None)?;

    let output = contynu_core::rendering::render_memory_export(&memories, max_chars, with_markers);
    print!("{output}");
    Ok(())
}

fn print_repair_result(project_id: &ProjectId, repair: contynu_core::JournalRepair) -> Result<()> {
    println!("Repair complete");
    println!();
    print_kv("Project", project_id.as_str());
    print_kv("Tail repaired", yes_no(repair.repaired));
    print_kv("Truncated at byte", &repair.truncated_at.to_string());
    Ok(())
}

fn print_pretty_json_value(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_kv(label: &str, value: &str) {
    println!("{label}: {value}");
}

fn print_optional_kv(label: &str, value: Option<&str>) {
    if let Some(value) = value {
        print_kv(label, value);
    }
}

fn print_section(title: &str, items: &[String]) {
    println!("{title}");
    println!();
    if items.is_empty() {
        println!("None recorded.");
    } else {
        for item in items {
            println!("- {}", one_line(item));
        }
    }
    println!();
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn summarize_json(value: &Value) -> String {
    let rendered = value.to_string();
    if rendered.len() > 160 {
        format!("{}...", &rendered[..160])
    } else {
        rendered
    }
}

fn format_turn_suffix(turn_id: Option<&contynu_core::TurnId>) -> String {
    turn_id
        .map(|turn| format!("  [{}]", turn))
        .unwrap_or_default()
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn hydration_delivery_name(delivery: contynu_core::HydrationDelivery) -> &'static str {
    match delivery {
        contynu_core::HydrationDelivery::EnvOnly => "env only",
        contynu_core::HydrationDelivery::StdinOnly => "stdin only",
        contynu_core::HydrationDelivery::EnvAndStdin => "env and stdin",
    }
}
