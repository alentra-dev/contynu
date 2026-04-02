use anyhow::Result;
use clap::{Parser, Subcommand};
use contynu_core::{Actor, BlobStore, EventEnvelope, Journal, MetadataStore, SessionId, TurnId};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "contynu")]
#[command(about = "Persistent memory layer for LLM workflows")]
struct Cli {
    #[arg(long, global = true, default_value = ".contynu")]
    state_dir: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init,
    StartSession,
    RecordMessage {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        turn_id: String,
        #[arg(long)]
        role: String,
        #[arg(long)]
        text: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init => init(&cli.state_dir),
        Command::StartSession => start_session(&cli.state_dir),
        Command::RecordMessage {
            session_id,
            turn_id,
            role,
            text,
        } => record_message(&cli.state_dir, &session_id, &turn_id, &role, &text),
    }
}

fn init(state_dir: &PathBuf) -> Result<()> {
    fs::create_dir_all(state_dir.join("journal"))?;
    fs::create_dir_all(state_dir.join("sqlite"))?;
    fs::create_dir_all(state_dir.join("blobs"))?;
    fs::create_dir_all(state_dir.join("checkpoints"))?;
    let _store = MetadataStore::open(state_dir.join("sqlite").join("contynu.db"))?;
    let _blob_store = BlobStore::new(state_dir.join("blobs"));
    println!("initialized Contynu state at {}", state_dir.display());
    Ok(())
}

fn start_session(state_dir: &PathBuf) -> Result<()> {
    init(state_dir)?;
    let session_id = SessionId::new();
    let store = MetadataStore::open(state_dir.join("sqlite").join("contynu.db"))?;
    store.register_session(&session_id, "started")?;
    println!("{}", session_id.as_str());
    Ok(())
}

fn record_message(
    state_dir: &PathBuf,
    session_id: &str,
    turn_id: &str,
    role: &str,
    text: &str,
) -> Result<()> {
    init(state_dir)?;
    let session = SessionId::from(session_id.to_string());
    let turn = TurnId::from(turn_id.to_string());
    let actor = match role {
        "user" => Actor::User,
        "assistant" => Actor::Assistant,
        _ => Actor::Runtime,
    };
    let mut event = EventEnvelope::new(
        session,
        turn,
        actor,
        format!("message_{}", role),
        json!({"content": [{"type": "text", "text": text}]}),
    );

    let journal_path = state_dir.join("journal").join(format!("{}.jsonl", session_id));
    let journal = Journal::open(&journal_path)?;
    journal.append(&mut event)?;

    let store = MetadataStore::open(state_dir.join("sqlite").join("contynu.db"))?;
    store.record_event(&event, &journal_path.display().to_string())?;
    println!("{}", event.event_id.as_str());
    Ok(())
}
