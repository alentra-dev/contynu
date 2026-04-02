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
    command: Command,
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
    maybe_reset_state_for_new(&state, &cli.command, cli.new)?;

    match cli.command {
        Command::Init => init(&state),
        Command::Run(command) => run(&state, &cli.cwd, command),
        Command::Codex(command) => launch_llm(&state, &cli.cwd, "codex", command),
        Command::Claude(command) => launch_llm(&state, &cli.cwd, "claude", command),
        Command::Gemini(command) => launch_llm(&state, &cli.cwd, "gemini", command),
        Command::Status { project } => status(&state, project.as_deref()),
        Command::Projects => projects(&state),
        Command::Recent { limit } => recent(&state, limit),
        Command::Config { command } => config_command(&state, command),
        Command::StartProject => start_project(&state, &cli.cwd),
        Command::Checkpoint { project, reason } => checkpoint(&state, project.as_deref(), &reason),
        Command::Resume { project } => resume(&state, project.as_deref(), None),
        Command::Handoff {
            project,
            target_model,
        } => resume(&state, project.as_deref(), Some(target_model)),
        Command::Replay { project } => replay(&state, project.as_deref()),
        Command::Inspect { command } => inspect(&state, command),
        Command::Search { command } => search(&state, command),
        Command::Artifacts { command } => artifacts(&state, command),
        Command::Doctor => doctor(&state),
        Command::Repair { project } => repair(&state, project.as_deref()),
        Command::External(command) => passthrough(&state, &cli.cwd, command),
    }
}

fn maybe_reset_state_for_new(state: &StatePaths, command: &Command, reset: bool) -> Result<()> {
    if !reset {
        return Ok(());
    }

    if !matches!(
        command,
        Command::Run(_)
            | Command::Codex(_)
            | Command::Claude(_)
            | Command::Gemini(_)
            | Command::StartProject
            | Command::External(_)
    ) {
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
                if let Some(context_file) = launcher.context_file.as_deref() {
                    println!("  context file: {}", context_file);
                }
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
