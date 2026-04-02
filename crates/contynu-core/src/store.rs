use crate::error::Result;
use crate::event::EventEnvelope;
use crate::ids::SessionId;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::Path;

pub struct MetadataStore {
    conn: Connection,
}

impl MetadataStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                project_id TEXT,
                status TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                cli_name TEXT,
                cli_version TEXT,
                model_name TEXT,
                cwd TEXT,
                repo_root TEXT,
                host_fingerprint TEXT
            );

            CREATE TABLE IF NOT EXISTS turns (
                turn_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                started_at TEXT NOT NULL,
                completed_at TEXT,
                summary_memory_id TEXT
            );

            CREATE TABLE IF NOT EXISTS events (
                event_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                ts TEXT NOT NULL,
                actor TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload_version INTEGER NOT NULL,
                journal_path TEXT,
                journal_byte_offset INTEGER,
                checksum TEXT,
                correlation_id TEXT,
                causation_id TEXT,
                UNIQUE(session_id, seq)
            );

            CREATE TABLE IF NOT EXISTS artifacts (
                artifact_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                path TEXT NOT NULL,
                kind TEXT NOT NULL,
                mime_type TEXT,
                sha256 TEXT NOT NULL,
                blob_id TEXT,
                created_at TEXT NOT NULL,
                deleted_at TEXT
            );

            CREATE TABLE IF NOT EXISTS checkpoints (
                checkpoint_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                reason TEXT NOT NULL,
                last_seq INTEGER NOT NULL,
                rehydration_blob_id TEXT
            );
            "#,
        )?;
        self.conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params!["schema_version", "1", Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn register_session(&self, session_id: &SessionId, status: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sessions (session_id, status, started_at) VALUES (?1, ?2, ?3)",
            params![session_id.as_str(), status, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn record_event(&self, event: &EventEnvelope, journal_path: &str) -> Result<()> {
        self.conn.execute(
            r#"INSERT OR REPLACE INTO events
            (event_id, session_id, turn_id, seq, ts, actor, event_type, payload_version, journal_path, checksum, correlation_id, causation_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
            params![
                event.event_id.as_str(),
                event.session_id.as_str(),
                event.turn_id.as_str(),
                event.seq,
                event.ts.to_rfc3339(),
                format!("{:?}", event.actor).to_lowercase(),
                event.event_type,
                event.payload_version,
                journal_path,
                event.checksum,
                event.correlation_id,
                event.causation_id.as_ref().map(|v| v.as_str())
            ],
        )?;
        Ok(())
    }
}
