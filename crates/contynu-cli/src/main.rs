use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand};
use contynu_core::{
    BlobStore, CheckpointManager, EventDraft, EventId, EventType, Journal, MetadataStore,
    ProjectId, RunConfig, RuntimeEngine, StatePaths,
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
    Codex(LlmCommand),
    Claude(LlmCommand),
    Gemini(LlmCommand),
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let state = StatePaths::new(&cli.state_dir);

    match cli.command {
        Command::Init => init(&state),
        Command::Run(command) => run(&state, &cli.cwd, command),
        Command::Codex(command) => launch_llm(&state, &cli.cwd, "codex", command),
        Command::Claude(command) => launch_llm(&state, &cli.cwd, "claude", command),
        Command::Gemini(command) => launch_llm(&state, &cli.cwd, "gemini", command),
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
    }
}

fn init(state: &StatePaths) -> Result<()> {
    ensure_state(state)?;
    println!("initialized {}", state.root().display());
    Ok(())
}

fn ensure_state(state: &StatePaths) -> Result<()> {
    state.ensure_layout()?;
    let _ = MetadataStore::open(state.sqlite_db())?;
    let _ = BlobStore::new(state.blobs_root());
    Ok(())
}

fn start_project(state: &StatePaths, cwd: &PathBuf) -> Result<()> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    if let Some(project_id) = store.primary_project_id()? {
        println!("{project_id}");
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
    println!("{project_id}");
    Ok(())
}

fn run(state: &StatePaths, cwd: &PathBuf, command: RunCommand) -> Result<()> {
    ensure_state(state)?;
    let outcome = RuntimeEngine::run(RunConfig {
        state_dir: state.root().to_path_buf(),
        cwd: cwd.clone(),
        command: command.command.into_iter().map(Into::into).collect(),
        ignore_patterns: command.ignore_patterns,
        checkpoint_on_exit: !command.no_checkpoint,
        project_id: command.project.map(ProjectId::parse).transpose()?,
    })?;
    print_json(&outcome)
}

fn launch_llm(
    state: &StatePaths,
    cwd: &PathBuf,
    executable: &str,
    command: LlmCommand,
) -> Result<()> {
    ensure_state(state)?;
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
    print_json(&outcome)
}

fn checkpoint(state: &StatePaths, project: Option<&str>, reason: &str) -> Result<()> {
    ensure_state(state)?;
    let project_id = resolve_project_id(state, project)?;
    let journal = Journal::open(state.journal_path_for_project(&project_id))?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let blobs = BlobStore::new(state.blobs_root());
    let manager = CheckpointManager::new(state, &store, &blobs);
    let (manifest, packet) = manager.create_checkpoint(&journal, &project_id, reason, None)?;
    print_json(&(manifest, packet))
}

fn resume(state: &StatePaths, project: Option<&str>, target_model: Option<String>) -> Result<()> {
    ensure_state(state)?;
    let project_id = resolve_project_id(state, project)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let blobs = BlobStore::new(state.blobs_root());
    let manager = CheckpointManager::new(state, &store, &blobs);
    let packet = manager.build_packet(&project_id, target_model)?;
    print_json(&packet)
}

fn replay(state: &StatePaths, project: Option<&str>) -> Result<()> {
    let project_id = resolve_project_id(state, project)?;
    let journal = Journal::open(state.journal_path_for_project(&project_id))?;
    let replay = journal.replay()?;
    print_json(&replay)
}

fn inspect(state: &StatePaths, command: InspectCommand) -> Result<()> {
    let store = MetadataStore::open(state.sqlite_db())?;
    match command {
        InspectCommand::Project { id } => {
            let project_id = match id {
                Some(id) => ProjectId::parse(id)?,
                None => resolve_primary_project(&store)?,
            };
            print_json(&store.list_events_for_session(&project_id)?)
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
        ArtifactsCommand::List { project } => {
            let project = project.map(ProjectId::parse).transpose()?;
            print_json(&store.list_artifacts(project.as_ref())?)
        }
    }
}

fn doctor(state: &StatePaths) -> Result<()> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let report = serde_json::json!({
        "state_root": state.root().display().to_string(),
        "sqlite_db": state.sqlite_db().display().to_string(),
        "journal_root": state.journal_root().display().to_string(),
        "artifacts": store.list_artifacts(None)?.len(),
    });
    print_json(&report)
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
            print_json(&repair)
        }
        None => {
            let project_id = resolve_project_id(state, None)?;
            let journal = Journal::open(state.journal_path_for_project(&project_id))?;
            let repair = journal.repair_truncated_tail()?;
            let store = MetadataStore::open(state.sqlite_db())?;
            store.reconcile_session(&journal, &project_id)?;
            print_json(&repair)
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

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
