mod mcp_registration;
mod mcp_server;
mod update_check;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand};
use contynu_core::{
    BlobStore, CheckpointManager, ContynuConfig, MetadataStore, ProjectId, RunConfig, RunOutcome,
    RuntimeEngine, SessionRecord, StatePaths,
};

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
    Search {
        #[command(subcommand)]
        command: SearchCommand,
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
    /// Discover and ingest unrecorded sessions from Claude Code, Codex, and Gemini
    Ingest {
        /// Show what would be ingested without actually doing it
        #[arg(long)]
        dry_run: bool,
        /// Only ingest from a specific tool (claude, codex, gemini)
        #[arg(long)]
        tool: Option<String>,
    },
    /// Dream Phase: scan for redundant memories and show consolidation candidates
    Distill {
        /// Project ID to distill (defaults to primary project)
        #[arg(long)]
        project: Option<String>,
    },
    Doctor,
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
enum SearchCommand {
    Memory { query: String },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Validate,
    Show,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if matches!(cli.command, Some(Command::McpServer { .. })) {
        // Skip interactive startup work for stdio MCP transport.
    } else {
        match update_check::maybe_handle_startup_update(false)? {
            update_check::StartupUpdateOutcome::Continue => {}
            update_check::StartupUpdateOutcome::ExitAfterManualPrompt
            | update_check::StartupUpdateOutcome::ExitAfterAutoUpdate => return Ok(()),
        }
    }
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
        Some(Command::Search { command }) => search(&state, command),
        Some(Command::Ingest { dry_run, tool }) => ingest(&state, &cli.cwd, dry_run, tool),
        Some(Command::Distill { project }) => distill(&state, project.as_deref()),
        Some(Command::Doctor) => doctor(&state),
        Some(Command::OpenClaw { command }) => openclaw_command(&state, &cli.cwd, command),
        Some(Command::ExportMemory {
            project,
            max_chars,
            with_markers,
        }) => export_memory(&state, project.as_deref(), max_chars, with_markers),
        Some(Command::McpServer {
            state_dir: override_dir,
        }) => {
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
    let store = MetadataStore::open(state.sqlite_db())?;
    // Purge any old architecture data on first access
    let _ = store.purge_old_data();
    let _ = state.cleanup_old_architecture();
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
    let project_id =
        reserve_project_for_llm_launch(state, cwd, executable, command.project.as_deref())?;

    // Auto-register MCP server for this CLI (non-fatal on failure)
    if let Err(e) =
        mcp_registration::ensure_mcp_registered(executable, state.root(), cwd, project_id.as_str())
    {
        eprintln!("Warning: MCP auto-registration: {e}");
    }

    let mut argv = vec![executable.to_string()];
    argv.extend(command.args);
    let outcome = RuntimeEngine::run(RunConfig {
        state_dir: state.root().to_path_buf(),
        cwd: cwd.clone(),
        command: argv.into_iter().map(Into::into).collect(),
        ignore_patterns: command.ignore_patterns,
        checkpoint_on_exit: !command.no_checkpoint,
        project_id: Some(project_id),
    })?;
    print_run_footer(&outcome);
    Ok(())
}

fn reserve_project_for_llm_launch(
    state: &StatePaths,
    cwd: &PathBuf,
    executable: &str,
    explicit_project: Option<&str>,
) -> Result<ProjectId> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;

    if let Some(project) = explicit_project {
        let project_id = ProjectId::parse(project)?;
        if !store.session_exists(&project_id)? {
            return Err(anyhow!("project `{project_id}` does not exist"));
        }
        if store.primary_project_id()?.as_ref() != Some(&project_id) {
            store.set_primary_project_id(&project_id)?;
        }
        return Ok(project_id);
    }

    if let Some(project_id) = store.primary_project_id()? {
        return Ok(project_id);
    }

    let project_id = ProjectId::new();
    store.register_session(&SessionRecord {
        session_id: project_id.clone(),
        project_id: Some(project_id.to_string()),
        status: "active".into(),
        cli_name: Some(executable.into()),
        cli_version: None,
        model_name: None,
        cwd: Some(cwd.display().to_string()),
        repo_root: Some(cwd.display().to_string()),
        host_fingerprint: None,
        started_at: chrono::Utc::now(),
        ended_at: None,
    })?;
    store.set_primary_project_id(&project_id)?;
    Ok(project_id)
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
    let store = MetadataStore::open(state.sqlite_db())?;
    let config = ContynuConfig::load(&state.config_path())?;
    let blobs = BlobStore::new(state.blobs_root());
    let manager = CheckpointManager::new(state, &store, &blobs);
    let (manifest, packet) =
        manager.create_checkpoint(&project_id, reason, None, &config.packet_budget.to_budget())?;
    print_checkpoint_result(&manifest, &packet)
}

fn resume(state: &StatePaths, project: Option<&str>, target_model: Option<String>) -> Result<()> {
    ensure_state(state)?;
    let project_id = resolve_project_id(state, project)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let config = ContynuConfig::load(&state.config_path())?;
    let blobs = BlobStore::new(state.blobs_root());
    let manager = CheckpointManager::new(state, &store, &blobs);
    let packet = manager.build_packet_with_budget(
        &project_id,
        target_model,
        &config.packet_budget.to_budget(),
    )?;
    print_rehydration_packet(
        if packet.target_model.is_some() {
            "Handoff Ready"
        } else {
            "Resume Ready"
        },
        &packet,
    )
}

fn search(state: &StatePaths, command: SearchCommand) -> Result<()> {
    let store = MetadataStore::open(state.sqlite_db())?;
    match command {
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
                    "- [{}] [{}] {} (importance: {:.2})",
                    memory.kind.as_str(),
                    memory.scope.as_str(),
                    memory.text,
                    memory.importance,
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
    let memory_count = store.count_active_memories(&project_id, None)?;
    println!("Project status");
    println!();
    print_kv("Project", project_id.as_str());
    print_kv("Status", &session.status);
    print_optional_kv("Launcher", session.cli_name.as_deref());
    print_optional_kv("Working directory", session.cwd.as_deref());
    print_kv("Started", &session.started_at.to_rfc3339());
    print_kv("Active memories", &memory_count.to_string());
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
    if let Some(ref pid) = primary_project {
        let mem_count = store.count_active_memories(pid, None)?;
        print_kv("Active memories", &mem_count.to_string());
    }
    print_kv(
        "Configured launchers",
        &config.llm_launchers.len().to_string(),
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
    let memories = store.list_active_memories(&project_id, None)?;

    let output = contynu_core::rendering::render_memory_export(&memories, max_chars, with_markers);
    print!("{output}");
    Ok(())
}

fn ingest(
    state: &StatePaths,
    cwd: &std::path::PathBuf,
    dry_run: bool,
    _tool_filter: Option<String>,
) -> Result<()> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let project_id = resolve_primary_project(&store)?;

    let cwd_abs = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.clone());
    let report = contynu_core::discovery::discover_all(&store, &cwd_abs)?;

    if report.total_new == 0 {
        println!("No new memories discovered from external AI tools.");
        return Ok(());
    }

    println!("Discovery results for {}:", cwd_abs.display());
    println!();

    if !report.claude_memories.is_empty() {
        println!(
            "  Claude Code: {} new memories",
            report.claude_memories.len()
        );
        for m in &report.claude_memories {
            println!(
                "    [{:>17}] {}",
                m.kind.as_str(),
                truncate_text(&m.text, 80)
            );
        }
    }
    if !report.codex_memories.is_empty() {
        println!(
            "  Codex:       {} new memories",
            report.codex_memories.len()
        );
        for m in &report.codex_memories {
            println!(
                "    [{:>17}] {}",
                m.kind.as_str(),
                truncate_text(&m.text, 80)
            );
        }
    }
    if !report.gemini_memories.is_empty() {
        println!(
            "  Gemini:      {} new memories",
            report.gemini_memories.len()
        );
        for m in &report.gemini_memories {
            println!(
                "    [{:>17}] {}",
                m.kind.as_str(),
                truncate_text(&m.text, 80)
            );
        }
    }

    println!();

    if dry_run {
        println!("Dry run — {} memories would be ingested.", report.total_new);
        return Ok(());
    }

    let ingested = contynu_core::discovery::ingest_memories(&store, &project_id, &report)?;
    println!(
        "Ingested {} memories into project {}.",
        ingested, project_id
    );
    Ok(())
}

fn truncate_text(text: &str, max_len: usize) -> String {
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.len() <= max_len {
        one
    } else {
        format!("{}...", &one[..max_len.saturating_sub(3)])
    }
}

fn distill(state: &StatePaths, project: Option<&str>) -> Result<()> {
    ensure_state(state)?;
    let store = MetadataStore::open(state.sqlite_db())?;
    let project_id = match project {
        Some(p) => ProjectId::parse(p).map_err(|e| anyhow!("{e}"))?,
        None => resolve_primary_project(&store)?,
    };

    let candidates = contynu_core::distiller::suggest_consolidation(&store, &project_id)?;

    if candidates.is_empty() {
        println!("No consolidation candidates found. Memory is clean.");
        return Ok(());
    }

    println!(
        "Dream Phase: {} consolidation candidates found\n",
        candidates.len()
    );

    for (i, candidate) in candidates.iter().enumerate() {
        println!(
            "  Cluster {} ({}, {} memories, {:.0}% similarity):",
            i + 1,
            candidate.kind.as_str(),
            candidate.memory_ids.len(),
            candidate.avg_similarity * 100.0,
        );
        for (j, text) in candidate.texts.iter().enumerate() {
            let id = candidate.memory_ids[j].as_str();
            let display = truncate_text(text, 70);
            println!("    [{id}] {display}");
        }
        println!();
    }

    println!("Use the consolidate_memories MCP tool to merge these clusters,");
    println!("or let your AI agent do it during the next session.");

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
    state.ensure_layout()?;
    contynu_core::ContynuConfig::ensure_exists(&state.root().join("config.json"))?;

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

    let oc_config = openclaw_config.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::path::PathBuf::from(home)
            .join(".openclaw")
            .join("openclaw.json")
    });
    if let Err(e) = mcp_registration::ensure_mcp_registered(
        "openclaw",
        state.root(),
        &oc_config,
        project_id.as_str(),
    ) {
        eprintln!("Warning: Could not register MCP server in OpenClaw config: {e}");
    }

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
    println!("  State directory:  {}", state.root().display());

    match store.primary_project_id()? {
        Some(id) => {
            println!("  Primary project:  {}", id);
            let total = store.count_active_memories(&id, None)?;
            println!("  Active memories:  {}", total);
        }
        None => println!("  Primary project:  (none)"),
    }

    let mapping_path = state.root().join("openclaw-agents.json");
    if mapping_path.exists() {
        let content = std::fs::read_to_string(&mapping_path)?;
        let map: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        let count = map.as_object().map_or(0, |m| m.len());
        println!("  Mapped agents:    {}", count);
    } else {
        println!("  Agent mapping:    (not created)");
    }

    let oc_config_path = std::env::var("HOME")
        .map(|h| {
            std::path::PathBuf::from(h)
                .join(".openclaw")
                .join("openclaw.json")
        })
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
            format!("Project {}.", short_id(outcome.project_id.as_str())),
        ]
    } else {
        vec![
            "Let's contynu another time. Goodbye for now.".to_string(),
            format!("Project {}.", short_id(outcome.project_id.as_str())),
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
    print_section("Recent context", &packet.recent_verbatim_context);
    print_section("Retrieval guidance", &packet.retrieval_guidance);
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
