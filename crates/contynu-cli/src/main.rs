use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand};
use contynu_core::{
    BlobStore, CheckpointManager, EventId, Journal, MetadataStore, RunConfig, RuntimeEngine,
    SessionId, StatePaths,
};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "contynu")]
#[command(about = "Local-first continuity engine for LLM workflows")]
struct Cli {
    #[arg(long, global = true, default_value = ".contynu")]
    state_dir: PathBuf,

    #[arg(long, global = true, default_value = ".")]
    cwd: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    Run(RunCommand),
    StartSession,
    Checkpoint {
        #[arg(long)]
        session: String,
        #[arg(long, default_value = "manual")]
        reason: String,
    },
    Resume {
        #[arg(long)]
        session: String,
    },
    Handoff {
        #[arg(long)]
        session: String,
        #[arg(long)]
        target_model: String,
    },
    Replay {
        #[arg(long)]
        session: String,
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
        #[arg(long)]
        session: Option<String>,
    },
}

#[derive(Debug, Args)]
struct RunCommand {
    #[arg(long)]
    no_checkpoint: bool,

    #[arg(long = "ignore")]
    ignore_patterns: Vec<String>,

    #[arg(last = true, required = true)]
    command: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum InspectCommand {
    Session { id: String },
    Event { id: String },
}

#[derive(Debug, Subcommand)]
enum SearchCommand {
    Exact { query: String },
    Memory { query: String },
}

#[derive(Debug, Subcommand)]
enum ArtifactsCommand {
    List {
        #[arg(long)]
        session: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let state = StatePaths::new(&cli.state_dir);

    match cli.command {
        Command::Init => init(&state),
        Command::Run(command) => run(&state, &cli.cwd, command),
        Command::StartSession => start_session(&state, &cli.cwd),
        Command::Checkpoint { session, reason } => checkpoint(&state, &session, &reason),
        Command::Resume { session } => resume(&state, &session, None),
        Command::Handoff {
            session,
            target_model,
        } => resume(&state, &session, Some(target_model)),
        Command::Replay { session } => replay(&state, &session),
        Command::Inspect { command } => inspect(&state, command),
        Command::Search { command } => search(&state, command),
        Command::Artifacts { command } => artifacts(&state, command),
        Command::Doctor => doctor(&state),
        Command::Repair { session } => repair(&state, session.as_deref()),
    }
}

fn init(state: &StatePaths) -> Result<()> {
    state.ensure_layout()?;
    let _ = MetadataStore::open(state.sqlite_db())?;
    let _ = BlobStore::new(state.blobs_root());
    println!("initialized {}", state.root().display());
    Ok(())
}

fn start_session(state: &StatePaths, cwd: &PathBuf) -> Result<()> {
    init(state)?;
    let session_id = SessionId::new();
    let store = MetadataStore::open(state.sqlite_db())?;
    store.register_session(&contynu_core::SessionRecord {
        session_id: session_id.clone(),
        project_id: None,
        status: "started".into(),
        cli_name: Some("manual".into()),
        cli_version: None,
        model_name: None,
        cwd: Some(cwd.display().to_string()),
        repo_root: Some(cwd.display().to_string()),
        host_fingerprint: None,
        started_at: chrono::Utc::now(),
        ended_at: None,
    })?;
    println!("{session_id}");
    Ok(())
}

fn run(state: &StatePaths, cwd: &PathBuf, command: RunCommand) -> Result<()> {
    init(state)?;
    let outcome = RuntimeEngine::run(RunConfig {
        state_dir: state.root().to_path_buf(),
        cwd: cwd.clone(),
        command: command.command.into_iter().map(Into::into).collect(),
        ignore_patterns: command.ignore_patterns,
        checkpoint_on_exit: !command.no_checkpoint,
    })?;
    print_json(&outcome)
}

fn checkpoint(state: &StatePaths, session: &str, reason: &str) -> Result<()> {
    init(state)?;
    let session_id = SessionId::parse(session.to_string())?;
    let journal = Journal::open(state.journal_path_for_session(&session_id))?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let blobs = BlobStore::new(state.blobs_root());
    let manager = CheckpointManager::new(state, &store, &blobs);
    let (manifest, packet) = manager.create_checkpoint(&journal, &session_id, reason, None)?;
    print_json(&(manifest, packet))
}

fn resume(state: &StatePaths, session: &str, target_model: Option<String>) -> Result<()> {
    init(state)?;
    let session_id = SessionId::parse(session.to_string())?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let blobs = BlobStore::new(state.blobs_root());
    let manager = CheckpointManager::new(state, &store, &blobs);
    let packet = manager.build_packet(&session_id, target_model)?;
    print_json(&packet)
}

fn replay(state: &StatePaths, session: &str) -> Result<()> {
    let session_id = SessionId::parse(session.to_string())?;
    let journal = Journal::open(state.journal_path_for_session(&session_id))?;
    let replay = journal.replay()?;
    print_json(&replay)
}

fn inspect(state: &StatePaths, command: InspectCommand) -> Result<()> {
    let store = MetadataStore::open(state.sqlite_db())?;
    match command {
        InspectCommand::Session { id } => {
            let session_id = SessionId::parse(id)?;
            print_json(&store.list_events_for_session(&session_id)?)
        }
        InspectCommand::Event { id } => {
            let event_id = EventId::parse(id)?;
            let event = store
                .get_event(&event_id)?
                .ok_or_else(|| anyhow!("event not found"))?;
            print_json(&event)
        }
    }
}

fn search(state: &StatePaths, command: SearchCommand) -> Result<()> {
    let store = MetadataStore::open(state.sqlite_db())?;
    match command {
        SearchCommand::Exact { query } => print_json(&store.search_exact(&query)?),
        SearchCommand::Memory { query } => print_json(&store.search_memory(&query)?),
    }
}

fn artifacts(state: &StatePaths, command: ArtifactsCommand) -> Result<()> {
    let store = MetadataStore::open(state.sqlite_db())?;
    match command {
        ArtifactsCommand::List { session } => {
            let session = session.map(SessionId::parse).transpose()?;
            print_json(&store.list_artifacts(session.as_ref())?)
        }
    }
}

fn doctor(state: &StatePaths) -> Result<()> {
    init(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let report = serde_json::json!({
        "state_root": state.root().display().to_string(),
        "sqlite_db": state.sqlite_db().display().to_string(),
        "journal_root": state.journal_root().display().to_string(),
        "artifacts": store.list_artifacts(None)?.len(),
    });
    print_json(&report)
}

fn repair(state: &StatePaths, session: Option<&str>) -> Result<()> {
    init(state)?;
    match session {
        Some(session) => {
            let session_id = SessionId::parse(session.to_string())?;
            let journal = Journal::open(state.journal_path_for_session(&session_id))?;
            let repair = journal.repair_truncated_tail()?;
            let store = MetadataStore::open(state.sqlite_db())?;
            store.reconcile_session(&journal, &session_id)?;
            print_json(&repair)
        }
        None => {
            let report = serde_json::json!({
                "status": "no-op",
                "detail": "pass --session to repair a specific journal"
            });
            print_json(&report)
        }
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
