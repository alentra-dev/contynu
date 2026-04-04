use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::blobs::BlobDescriptor;
use crate::checkpoint::CheckpointManifest;
use crate::error::{ContynuError, Result};
use crate::event::EventEnvelope;
use crate::ids::{ArtifactId, CheckpointId, EventId, FileId, MemoryId, SessionId, TurnId};
use crate::journal::{Journal, JournalAppend};

const MIGRATION_1: &str = include_str!("../../../sql/metadata_schema.sql");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: SessionId,
    pub project_id: Option<String>,
    pub status: String,
    pub cli_name: Option<String>,
    pub cli_version: Option<String>,
    pub model_name: Option<String>,
    pub cwd: Option<String>,
    pub repo_root: Option<String>,
    pub host_fingerprint: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

pub type ProjectRecord = SessionRecord;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub summary_memory_id: Option<MemoryId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub event_id: EventId,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub actor: String,
    pub event_type: String,
    pub payload_json: Value,
    pub checksum: String,
    pub journal_path: String,
    pub journal_byte_offset: u64,
    pub journal_line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub artifact_id: ArtifactId,
    pub session_id: SessionId,
    pub source_event_id: EventId,
    pub path: Option<String>,
    pub kind: String,
    pub mime_type: Option<String>,
    pub sha256: String,
    pub blob_relative_path: String,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub file_id: FileId,
    pub session_id: SessionId,
    pub workspace_relative_path: String,
    pub kind: String,
    pub last_known_sha256: Option<String>,
    pub last_snapshot_event_id: Option<EventId>,
    pub last_diff_event_id: Option<EventId>,
    pub observed_at: DateTime<Utc>,
    pub is_generated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointRecord {
    pub checkpoint_id: CheckpointId,
    pub session_id: SessionId,
    pub source_event_id: EventId,
    pub reason: String,
    pub last_seq: u64,
    pub rehydration_sha256: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryObjectKind {
    Fact,
    Constraint,
    Decision,
    Todo,
    Summary,
    Entity,
    FileNote,
}

impl MemoryObjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Constraint => "constraint",
            Self::Decision => "decision",
            Self::Todo => "todo",
            Self::Summary => "summary",
            Self::Entity => "entity",
            Self::FileNote => "file_note",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryObject {
    pub memory_id: MemoryId,
    pub session_id: SessionId,
    pub kind: MemoryObjectKind,
    pub status: String,
    pub text: String,
    pub confidence: Option<f64>,
    pub source_event_ids: Vec<EventId>,
    pub created_at: DateTime<Utc>,
    pub superseded_by: Option<MemoryId>,
    pub source_adapter: Option<String>,
    pub source_model: Option<String>,
    pub importance: f64,
    pub access_count: u32,
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub consolidated_from: Vec<MemoryId>,
    pub text_hash: Option<String>,
}

/// Query parameters for flexible memory search.
pub struct MemoryQuery {
    pub session_id: Option<SessionId>,
    pub text_query: Option<String>,
    pub kind: Option<MemoryObjectKind>,
    pub source_adapter: Option<String>,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub sort_by: MemorySortBy,
    pub limit: usize,
    pub offset: usize,
}

impl Default for MemoryQuery {
    fn default() -> Self {
        Self {
            session_id: None,
            text_query: None,
            kind: None,
            source_adapter: None,
            after: None,
            before: None,
            sort_by: MemorySortBy::Importance,
            limit: 20,
            offset: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MemorySortBy {
    Importance,
    Recency,
}

/// Query parameters for flexible event search.
pub struct EventQuery {
    pub session_id: Option<SessionId>,
    pub text_query: Option<String>,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: usize,
    pub offset: usize,
}

impl Default for EventQuery {
    fn default() -> Self {
        Self {
            session_id: None,
            text_query: None,
            after: None,
            before: None,
            limit: 20,
            offset: 0,
        }
    }
}

pub struct MetadataStore {
    conn: Connection,
}

const PRIMARY_PROJECT_KEY: &str = "primary_project_id";

impl MetadataStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute_batch("PRAGMA query_only = ON;")?;
        Ok(Self { conn })
    }

    pub fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_migrations (
              version INTEGER PRIMARY KEY,
              applied_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS schema_meta (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            "#,
        )?;

        let applied = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        if applied == 0 {
            self.conn.execute_batch(MIGRATION_1)?;
            self.conn.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![1_i64, Utc::now().to_rfc3339()],
            )?;
        }

        let applied_v2 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 2",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        if applied_v2 == 0 {
            self.conn.execute_batch(
                r#"
                ALTER TABLE memory_objects ADD COLUMN source_adapter TEXT;
                ALTER TABLE memory_objects ADD COLUMN source_model TEXT;
                ALTER TABLE memory_objects ADD COLUMN importance REAL DEFAULT 0.5;
                ALTER TABLE memory_objects ADD COLUMN access_count INTEGER DEFAULT 0;
                ALTER TABLE memory_objects ADD COLUMN last_accessed_at TEXT;
                ALTER TABLE memory_objects ADD COLUMN consolidated_from_json TEXT;
                ALTER TABLE memory_objects ADD COLUMN text_hash TEXT;

                CREATE INDEX IF NOT EXISTS idx_memory_active_importance
                  ON memory_objects(session_id, status, importance DESC, created_at DESC);

                CREATE INDEX IF NOT EXISTS idx_memory_text_hash
                  ON memory_objects(session_id, kind, text_hash);
                "#,
            )?;
            self.conn.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![2_i64, Utc::now().to_rfc3339()],
            )?;
        }

        self.conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params!["schema_version", "2", Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn register_session(&self, session: &SessionRecord) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO sessions (
              session_id, project_id, status, cli_name, cli_version, model_name, cwd,
              repo_root, host_fingerprint, started_at, ended_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            "#,
            params![
                session.session_id.as_str(),
                session.project_id,
                session.status,
                session.cli_name,
                session.cli_version,
                session.model_name,
                session.cwd,
                session.repo_root,
                session.host_fingerprint,
                session.started_at.to_rfc3339(),
                session.ended_at.map(|value| value.to_rfc3339())
            ],
        )?;
        Ok(())
    }

    pub fn primary_project_id(&self) -> Result<Option<SessionId>> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = ?1",
                params![PRIMARY_PROJECT_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        value.map(SessionId::parse).transpose()
    }

    pub fn set_primary_project_id(&self, session_id: &SessionId) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params![
                PRIMARY_PROJECT_KEY,
                session_id.as_str(),
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn get_session(&self, session_id: &SessionId) -> Result<Option<SessionRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT session_id, project_id, status, cli_name, cli_version, model_name, cwd,
                   repo_root, host_fingerprint, started_at, ended_at
            FROM sessions
            WHERE session_id = ?1
            "#,
        )?;
        let session = stmt
            .query_row(params![session_id.as_str()], |row| {
                Ok(SessionRecord {
                    session_id: SessionId::parse(row.get::<_, String>(0)?)
                        .map_err(into_sql_error)?,
                    project_id: row.get(1)?,
                    status: row.get(2)?,
                    cli_name: row.get(3)?,
                    cli_version: row.get(4)?,
                    model_name: row.get(5)?,
                    cwd: row.get(6)?,
                    repo_root: row.get(7)?,
                    host_fingerprint: row.get(8)?,
                    started_at: parse_rfc3339(&row.get::<_, String>(9)?).map_err(into_sql_error)?,
                    ended_at: row
                        .get::<_, Option<String>>(10)?
                        .map(|value| parse_rfc3339(&value))
                        .transpose()
                        .map_err(into_sql_error)?,
                })
            })
            .optional()?;
        Ok(session)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT session_id, project_id, status, cli_name, cli_version, model_name, cwd,
                   repo_root, host_fingerprint, started_at, ended_at
            FROM sessions
            ORDER BY started_at DESC
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SessionRecord {
                session_id: SessionId::parse(row.get::<_, String>(0)?).map_err(into_sql_error)?,
                project_id: row.get(1)?,
                status: row.get(2)?,
                cli_name: row.get(3)?,
                cli_version: row.get(4)?,
                model_name: row.get(5)?,
                cwd: row.get(6)?,
                repo_root: row.get(7)?,
                host_fingerprint: row.get(8)?,
                started_at: parse_rfc3339(&row.get::<_, String>(9)?).map_err(into_sql_error)?,
                ended_at: row
                    .get::<_, Option<String>>(10)?
                    .map(|value| parse_rfc3339(&value))
                    .transpose()
                    .map_err(into_sql_error)?,
            })
        })?;
        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    pub fn update_session_status(
        &self,
        session_id: &SessionId,
        status: &str,
        ended_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status = ?2, ended_at = ?3 WHERE session_id = ?1",
            params![
                session_id.as_str(),
                status,
                ended_at.map(|value| value.to_rfc3339())
            ],
        )?;
        Ok(())
    }

    pub fn register_turn(&self, turn: &TurnRecord) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO turns (
              turn_id, session_id, status, started_at, completed_at, summary_memory_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                turn.turn_id.as_str(),
                turn.session_id.as_str(),
                turn.status,
                turn.started_at.to_rfc3339(),
                turn.completed_at.map(|value| value.to_rfc3339()),
                turn.summary_memory_id.clone().map(String::from)
            ],
        )?;
        Ok(())
    }

    pub fn update_turn_status(
        &self,
        turn_id: &TurnId,
        status: &str,
        completed_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE turns SET status = ?2, completed_at = ?3 WHERE turn_id = ?1",
            params![
                turn_id.as_str(),
                status,
                completed_at.map(|value| value.to_rfc3339())
            ],
        )?;
        Ok(())
    }

    pub fn list_turns_for_session(&self, session_id: &SessionId) -> Result<Vec<TurnRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT turn_id, session_id, status, started_at, completed_at, summary_memory_id
            FROM turns
            WHERE session_id = ?1
            ORDER BY started_at DESC
            "#,
        )?;
        let rows = stmt.query_map(params![session_id.as_str()], |row| {
            Ok(TurnRecord {
                turn_id: TurnId::parse(row.get::<_, String>(0)?).map_err(into_sql_error)?,
                session_id: SessionId::parse(row.get::<_, String>(1)?).map_err(into_sql_error)?,
                status: row.get(2)?,
                started_at: parse_rfc3339(&row.get::<_, String>(3)?).map_err(into_sql_error)?,
                completed_at: row
                    .get::<_, Option<String>>(4)?
                    .map(|value| parse_rfc3339(&value))
                    .transpose()
                    .map_err(into_sql_error)?,
                summary_memory_id: row
                    .get::<_, Option<String>>(5)?
                    .map(MemoryId::parse)
                    .transpose()
                    .map_err(into_sql_error)?,
            })
        })?;
        let mut turns = Vec::new();
        for row in rows {
            turns.push(row?);
        }
        Ok(turns)
    }

    pub fn set_turn_summary_memory(
        &self,
        turn_id: &TurnId,
        summary_memory_id: &MemoryId,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE turns SET summary_memory_id = ?2 WHERE turn_id = ?1",
            params![turn_id.as_str(), summary_memory_id.as_str()],
        )?;
        Ok(())
    }

    pub fn record_event(
        &self,
        event: &EventEnvelope,
        journal_path: &str,
        append: JournalAppend,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO events (
              event_id, session_id, turn_id, seq, ts, actor, event_type, payload_version,
              payload_json, checksum, parent_event_id, correlation_id, causation_id, tags_json,
              journal_path, journal_byte_offset, journal_line
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            "#,
            params![
                event.event_id.as_str(),
                event.session_id.as_str(),
                event.turn_id.as_ref().map(|id| id.as_str()),
                event.seq,
                event.ts.to_rfc3339(),
                event.actor.as_str(),
                event.event_type.as_str(),
                event.payload_version,
                serde_json::to_string(&event.payload)?,
                event.checksum,
                event.parent_event_id.as_ref().map(|id| id.as_str()),
                event.correlation_id,
                event.causation_id.as_ref().map(|id| id.as_str()),
                serde_json::to_string(&event.tags)?,
                journal_path,
                append.byte_offset,
                append.line_number as i64
            ],
        )?;
        Ok(())
    }

    pub fn register_blob(&self, blob: &BlobDescriptor, mime_type: Option<&str>) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO blobs (blob_id, sha256, size_bytes, mime_type, storage_path, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                blob.sha256,
                blob.sha256,
                blob.size_bytes as i64,
                mime_type,
                blob.relative_path,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn register_artifact(&self, record: &ArtifactRecord) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO artifacts (
              artifact_id, session_id, source_event_id, path, kind, mime_type, sha256,
              blob_relative_path, size_bytes, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                record.artifact_id.as_str(),
                record.session_id.as_str(),
                record.source_event_id.as_str(),
                record.path,
                record.kind,
                record.mime_type,
                record.sha256,
                record.blob_relative_path,
                record.size_bytes as i64,
                record.created_at.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn register_file(&self, file: &FileRecord) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO files (
              file_id, session_id, workspace_relative_path, kind, last_known_sha256,
              last_snapshot_event_id, last_diff_event_id, observed_at, is_generated
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                file.file_id.as_str(),
                file.session_id.as_str(),
                file.workspace_relative_path,
                file.kind,
                file.last_known_sha256,
                file.last_snapshot_event_id.as_ref().map(|id| id.as_str()),
                file.last_diff_event_id.as_ref().map(|id| id.as_str()),
                file.observed_at.to_rfc3339(),
                if file.is_generated { 1 } else { 0 }
            ],
        )?;
        Ok(())
    }

    pub fn register_checkpoint(
        &self,
        checkpoint: &CheckpointRecord,
        manifest: &CheckpointManifest,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO checkpoints (
              checkpoint_id, session_id, source_event_id, reason, last_seq,
              rehydration_sha256, manifest_json, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                checkpoint.checkpoint_id.as_str(),
                checkpoint.session_id.as_str(),
                checkpoint.source_event_id.as_str(),
                checkpoint.reason,
                checkpoint.last_seq as i64,
                checkpoint.rehydration_sha256,
                serde_json::to_string(manifest)?,
                checkpoint.created_at.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn insert_memory_object(&self, memory: &MemoryObject) -> Result<()> {
        let source_event_ids = memory
            .source_event_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let consolidated_from = memory
            .consolidated_from
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO memory_objects (
              memory_id, session_id, kind, status, text, confidence, source_event_ids_json,
              created_at, superseded_by,
              source_adapter, source_model, importance, access_count, last_accessed_at,
              consolidated_from_json, text_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            "#,
            params![
                memory.memory_id.as_str(),
                memory.session_id.as_str(),
                memory.kind.as_str(),
                memory.status,
                memory.text,
                memory.confidence,
                serde_json::to_string(&source_event_ids)?,
                memory.created_at.to_rfc3339(),
                memory.superseded_by.clone().map(String::from),
                memory.source_adapter,
                memory.source_model,
                memory.importance,
                memory.access_count,
                memory.last_accessed_at.map(|dt| dt.to_rfc3339()),
                serde_json::to_string(&consolidated_from)?,
                memory.text_hash,
            ],
        )?;
        Ok(())
    }

    pub fn supersede_memory_kind(
        &self,
        session_id: &SessionId,
        kind: MemoryObjectKind,
        superseded_by: &MemoryId,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            UPDATE memory_objects
            SET status = 'superseded', superseded_by = ?3
            WHERE session_id = ?1 AND kind = ?2 AND status != 'superseded'
            "#,
            params![session_id.as_str(), kind.as_str(), superseded_by.as_str()],
        )?;
        Ok(())
    }

    pub fn supersede_memory_kind_single(
        &self,
        _session_id: &SessionId,
        memory_id: &MemoryId,
        superseded_by: &MemoryId,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            UPDATE memory_objects
            SET status = 'superseded', superseded_by = ?2
            WHERE memory_id = ?1 AND status != 'superseded'
            "#,
            params![memory_id.as_str(), superseded_by.as_str()],
        )?;
        Ok(())
    }

    pub fn find_active_memory_by_text(
        &self,
        session_id: &SessionId,
        kind: MemoryObjectKind,
        text: &str,
    ) -> Result<Option<MemoryObject>> {
        let sql = format!(
            "SELECT {MEMORY_SELECT_COLUMNS} FROM memory_objects
             WHERE session_id = ?1 AND kind = ?2 AND text = ?3 AND status != 'superseded'
             ORDER BY created_at DESC
             LIMIT 1"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let memory = stmt
            .query_row(
                params![session_id.as_str(), kind.as_str(), text],
                map_memory,
            )
            .optional()?;
        Ok(memory)
    }

    pub fn list_events_for_session(&self, session_id: &SessionId) -> Result<Vec<EventRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT event_id, session_id, turn_id, seq, ts, actor, event_type, payload_json,
                   checksum, journal_path, journal_byte_offset, journal_line
            FROM events
            WHERE session_id = ?1
            ORDER BY seq ASC
            "#,
        )?;
        let rows = stmt.query_map(params![session_id.as_str()], |row| {
            let event_id: String = row.get(0)?;
            let session_id: String = row.get(1)?;
            let turn_id: Option<String> = row.get(2)?;
            let payload_json: String = row.get(7)?;
            Ok(EventRecord {
                event_id: EventId::parse(event_id).map_err(into_sql_error)?,
                session_id: SessionId::parse(session_id).map_err(into_sql_error)?,
                turn_id: turn_id
                    .map(TurnId::parse)
                    .transpose()
                    .map_err(into_sql_error)?,
                seq: row.get::<_, i64>(3)? as u64,
                ts: parse_rfc3339(&row.get::<_, String>(4)?).map_err(into_sql_error)?,
                actor: row.get(5)?,
                event_type: row.get(6)?,
                payload_json: serde_json::from_str(&payload_json).map_err(into_sql_error)?,
                checksum: row.get(8)?,
                journal_path: row.get(9)?,
                journal_byte_offset: row.get::<_, i64>(10)? as u64,
                journal_line: row.get::<_, i64>(11)? as usize,
            })
        })?;

        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn get_event(&self, event_id: &EventId) -> Result<Option<EventRecord>> {
        let mut events = self.conn.prepare(
            r#"
            SELECT event_id, session_id, turn_id, seq, ts, actor, event_type, payload_json,
                   checksum, journal_path, journal_byte_offset, journal_line
            FROM events WHERE event_id = ?1
            "#,
        )?;
        let event = events
            .query_row(params![event_id.as_str()], |row| {
                let payload_json: String = row.get(7)?;
                let turn_id: Option<String> = row.get(2)?;
                Ok(EventRecord {
                    event_id: EventId::parse(row.get::<_, String>(0)?).map_err(into_sql_error)?,
                    session_id: SessionId::parse(row.get::<_, String>(1)?)
                        .map_err(into_sql_error)?,
                    turn_id: turn_id
                        .map(TurnId::parse)
                        .transpose()
                        .map_err(into_sql_error)?,
                    seq: row.get::<_, i64>(3)? as u64,
                    ts: parse_rfc3339(&row.get::<_, String>(4)?).map_err(into_sql_error)?,
                    actor: row.get(5)?,
                    event_type: row.get(6)?,
                    payload_json: serde_json::from_str(&payload_json).map_err(into_sql_error)?,
                    checksum: row.get(8)?,
                    journal_path: row.get(9)?,
                    journal_byte_offset: row.get::<_, i64>(10)? as u64,
                    journal_line: row.get::<_, i64>(11)? as usize,
                })
            })
            .optional()?;
        Ok(event)
    }

    pub fn list_artifacts(&self, session_id: Option<&SessionId>) -> Result<Vec<ArtifactRecord>> {
        let sql = if session_id.is_some() {
            r#"
            SELECT artifact_id, session_id, source_event_id, path, kind, mime_type, sha256,
                   blob_relative_path, size_bytes, created_at
            FROM artifacts
            WHERE session_id = ?1
            ORDER BY created_at ASC
            "#
        } else {
            r#"
            SELECT artifact_id, session_id, source_event_id, path, kind, mime_type, sha256,
                   blob_relative_path, size_bytes, created_at
            FROM artifacts
            ORDER BY created_at ASC
            "#
        };
        let mut stmt = self.conn.prepare(sql)?;
        let mapper = |row: &rusqlite::Row<'_>| {
            Ok(ArtifactRecord {
                artifact_id: ArtifactId::parse(row.get::<_, String>(0)?).map_err(into_sql_error)?,
                session_id: SessionId::parse(row.get::<_, String>(1)?).map_err(into_sql_error)?,
                source_event_id: EventId::parse(row.get::<_, String>(2)?)
                    .map_err(into_sql_error)?,
                path: row.get(3)?,
                kind: row.get(4)?,
                mime_type: row.get(5)?,
                sha256: row.get(6)?,
                blob_relative_path: row.get(7)?,
                size_bytes: row.get::<_, i64>(8)? as u64,
                created_at: parse_rfc3339(&row.get::<_, String>(9)?).map_err(into_sql_error)?,
            })
        };
        let rows = if let Some(session_id) = session_id {
            stmt.query_map(params![session_id.as_str()], mapper)?
        } else {
            stmt.query_map([], mapper)?
        };

        let mut artifacts = Vec::new();
        for row in rows {
            artifacts.push(row?);
        }
        Ok(artifacts)
    }

    pub fn list_current_files(&self, session_id: &SessionId) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT file_id, session_id, workspace_relative_path, kind, last_known_sha256,
                   last_snapshot_event_id, last_diff_event_id, observed_at, is_generated
            FROM files
            WHERE session_id = ?1
            ORDER BY workspace_relative_path ASC, observed_at DESC
            "#,
        )?;
        let rows = stmt.query_map(params![session_id.as_str()], |row| {
            Ok(FileRecord {
                file_id: FileId::parse(row.get::<_, String>(0)?).map_err(into_sql_error)?,
                session_id: SessionId::parse(row.get::<_, String>(1)?).map_err(into_sql_error)?,
                workspace_relative_path: row.get(2)?,
                kind: row.get(3)?,
                last_known_sha256: row.get(4)?,
                last_snapshot_event_id: row
                    .get::<_, Option<String>>(5)?
                    .map(EventId::parse)
                    .transpose()
                    .map_err(into_sql_error)?,
                last_diff_event_id: row
                    .get::<_, Option<String>>(6)?
                    .map(EventId::parse)
                    .transpose()
                    .map_err(into_sql_error)?,
                observed_at: parse_rfc3339(&row.get::<_, String>(7)?).map_err(into_sql_error)?,
                is_generated: row.get::<_, i64>(8)? != 0,
            })
        })?;

        let mut deduped = Vec::new();
        let mut last_path = None::<String>;
        for row in rows {
            let file = row?;
            if last_path.as_deref() == Some(file.workspace_relative_path.as_str()) {
                continue;
            }
            last_path = Some(file.workspace_relative_path.clone());
            deduped.push(file);
        }
        Ok(deduped)
    }

    pub fn list_events_for_turn(
        &self,
        session_id: &SessionId,
        turn_id: &TurnId,
    ) -> Result<Vec<EventRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT event_id, session_id, turn_id, seq, ts, actor, event_type, payload_json,
                   checksum, journal_path, journal_byte_offset, journal_line
            FROM events
            WHERE session_id = ?1 AND turn_id = ?2
            ORDER BY seq ASC
            "#,
        )?;
        let rows = stmt.query_map(params![session_id.as_str(), turn_id.as_str()], |row| {
            let payload_json: String = row.get(7)?;
            Ok(EventRecord {
                event_id: EventId::parse(row.get::<_, String>(0)?).map_err(into_sql_error)?,
                session_id: SessionId::parse(row.get::<_, String>(1)?).map_err(into_sql_error)?,
                turn_id: row
                    .get::<_, Option<String>>(2)?
                    .map(TurnId::parse)
                    .transpose()
                    .map_err(into_sql_error)?,
                seq: row.get::<_, i64>(3)? as u64,
                ts: parse_rfc3339(&row.get::<_, String>(4)?).map_err(into_sql_error)?,
                actor: row.get(5)?,
                event_type: row.get(6)?,
                payload_json: serde_json::from_str(&payload_json).map_err(into_sql_error)?,
                checksum: row.get(8)?,
                journal_path: row.get(9)?,
                journal_byte_offset: row.get::<_, i64>(10)? as u64,
                journal_line: row.get::<_, i64>(11)? as usize,
            })
        })?;

        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn list_memory_objects(
        &self,
        session_id: &SessionId,
        kind: Option<MemoryObjectKind>,
    ) -> Result<Vec<MemoryObject>> {
        let sql = if kind.is_some() {
            format!(
                "SELECT {MEMORY_SELECT_COLUMNS} FROM memory_objects
                 WHERE session_id = ?1 AND kind = ?2
                 ORDER BY created_at ASC"
            )
        } else {
            format!(
                "SELECT {MEMORY_SELECT_COLUMNS} FROM memory_objects
                 WHERE session_id = ?1
                 ORDER BY created_at ASC"
            )
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = if let Some(kind) = kind {
            stmt.query_map(params![session_id.as_str(), kind.as_str()], map_memory)?
        } else {
            stmt.query_map(params![session_id.as_str()], map_memory)?
        };

        let mut memory = Vec::new();
        for row in rows {
            memory.push(row?);
        }
        Ok(memory)
    }

    pub fn list_active_memory_objects(
        &self,
        session_id: &SessionId,
        kind: Option<MemoryObjectKind>,
    ) -> Result<Vec<MemoryObject>> {
        let sql = if kind.is_some() {
            format!(
                "SELECT {MEMORY_SELECT_COLUMNS} FROM memory_objects
                 WHERE session_id = ?1 AND kind = ?2 AND status != 'superseded'
                 ORDER BY importance DESC, created_at DESC"
            )
        } else {
            format!(
                "SELECT {MEMORY_SELECT_COLUMNS} FROM memory_objects
                 WHERE session_id = ?1 AND status != 'superseded'
                 ORDER BY importance DESC, created_at DESC"
            )
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = if let Some(kind) = kind {
            stmt.query_map(params![session_id.as_str(), kind.as_str()], map_memory)?
        } else {
            stmt.query_map(params![session_id.as_str()], map_memory)?
        };

        let mut memory = Vec::new();
        for row in rows {
            memory.push(row?);
        }
        Ok(memory)
    }

    pub fn search_exact(&self, query: &str) -> Result<Vec<EventRecord>> {
        let needle = format!("%{query}%");
        let mut stmt = self.conn.prepare(
            r#"
            SELECT event_id, session_id, turn_id, seq, ts, actor, event_type, payload_json,
                   checksum, journal_path, journal_byte_offset, journal_line
            FROM events
            WHERE payload_json LIKE ?1 OR event_type LIKE ?1
            ORDER BY ts DESC
            LIMIT 50
            "#,
        )?;
        let rows = stmt.query_map(params![needle], |row| {
            let payload_json: String = row.get(7)?;
            let turn_id: Option<String> = row.get(2)?;
            Ok(EventRecord {
                event_id: EventId::parse(row.get::<_, String>(0)?).map_err(into_sql_error)?,
                session_id: SessionId::parse(row.get::<_, String>(1)?).map_err(into_sql_error)?,
                turn_id: turn_id
                    .map(TurnId::parse)
                    .transpose()
                    .map_err(into_sql_error)?,
                seq: row.get::<_, i64>(3)? as u64,
                ts: parse_rfc3339(&row.get::<_, String>(4)?).map_err(into_sql_error)?,
                actor: row.get(5)?,
                event_type: row.get(6)?,
                payload_json: serde_json::from_str(&payload_json).map_err(into_sql_error)?,
                checksum: row.get(8)?,
                journal_path: row.get(9)?,
                journal_byte_offset: row.get::<_, i64>(10)? as u64,
                journal_line: row.get::<_, i64>(11)? as usize,
            })
        })?;

        let mut matches = Vec::new();
        for row in rows {
            matches.push(row?);
        }
        Ok(matches)
    }

    pub fn search_memory(&self, query: &str) -> Result<Vec<MemoryObject>> {
        let needle = format!("%{query}%");
        let sql = format!(
            "SELECT {MEMORY_SELECT_COLUMNS} FROM memory_objects
             WHERE text LIKE ?1 AND status != 'superseded'
             ORDER BY created_at DESC
             LIMIT 50"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![needle], map_memory)?;
        let mut matches = Vec::new();
        for row in rows {
            matches.push(row?);
        }
        Ok(matches)
    }

    /// Flexible memory search with filtering, pagination, and sorting.
    pub fn query_memories(&self, query: &MemoryQuery) -> Result<Vec<MemoryObject>> {
        let mut conditions = vec!["status != 'superseded'".to_string()];
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut param_idx = 1;

        if let Some(ref session_id) = query.session_id {
            conditions.push(format!("session_id = ?{param_idx}"));
            param_values.push(Box::new(session_id.as_str().to_string()));
            param_idx += 1;
        }
        if let Some(ref text) = query.text_query {
            conditions.push(format!("text LIKE ?{param_idx}"));
            param_values.push(Box::new(format!("%{text}%")));
            param_idx += 1;
        }
        if let Some(ref kind) = query.kind {
            conditions.push(format!("kind = ?{param_idx}"));
            param_values.push(Box::new(kind.as_str().to_string()));
            param_idx += 1;
        }
        if let Some(ref adapter) = query.source_adapter {
            conditions.push(format!("source_adapter = ?{param_idx}"));
            param_values.push(Box::new(adapter.clone()));
            param_idx += 1;
        }
        if let Some(ref after) = query.after {
            conditions.push(format!("created_at >= ?{param_idx}"));
            param_values.push(Box::new(after.to_rfc3339()));
            param_idx += 1;
        }
        if let Some(ref before) = query.before {
            conditions.push(format!("created_at <= ?{param_idx}"));
            param_values.push(Box::new(before.to_rfc3339()));
            let _ = param_idx;
        }

        let order = match query.sort_by {
            MemorySortBy::Importance => "importance DESC, created_at DESC",
            MemorySortBy::Recency => "created_at DESC",
        };

        let limit = query.limit.min(50).max(1);
        let offset = query.offset;

        let sql = format!(
            "SELECT {MEMORY_SELECT_COLUMNS} FROM memory_objects WHERE {} ORDER BY {} LIMIT {} OFFSET {}",
            conditions.join(" AND "),
            order,
            limit,
            offset
        );

        let params: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params.as_slice(), map_memory)?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Flexible event search with filtering and pagination.
    pub fn query_events(&self, query: &EventQuery) -> Result<Vec<EventRecord>> {
        let mut conditions = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut param_idx = 1;

        if let Some(ref text) = query.text_query {
            conditions.push(format!("(payload_json LIKE ?{param_idx} OR event_type LIKE ?{param_idx})"));
            param_values.push(Box::new(format!("%{text}%")));
            param_idx += 1;
        }
        if let Some(ref session_id) = query.session_id {
            conditions.push(format!("session_id = ?{param_idx}"));
            param_values.push(Box::new(session_id.as_str().to_string()));
            param_idx += 1;
        }
        if let Some(ref after) = query.after {
            conditions.push(format!("ts >= ?{param_idx}"));
            param_values.push(Box::new(after.to_rfc3339()));
            param_idx += 1;
        }
        if let Some(ref before) = query.before {
            conditions.push(format!("ts <= ?{param_idx}"));
            param_values.push(Box::new(before.to_rfc3339()));
            let _ = param_idx;
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let limit = query.limit.min(50).max(1);
        let offset = query.offset;

        let sql = format!(
            r#"SELECT event_id, session_id, turn_id, seq, ts, actor, event_type, payload_json,
                      checksum, journal_path, journal_byte_offset, journal_line
               FROM events {where_clause}
               ORDER BY ts DESC
               LIMIT {limit} OFFSET {offset}"#
        );

        let params: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params.as_slice(), |row| {
            let payload_json: String = row.get(7)?;
            let turn_id: Option<String> = row.get(2)?;
            Ok(EventRecord {
                event_id: EventId::parse(row.get::<_, String>(0)?).map_err(into_sql_error)?,
                session_id: SessionId::parse(row.get::<_, String>(1)?).map_err(into_sql_error)?,
                turn_id: turn_id
                    .map(TurnId::parse)
                    .transpose()
                    .map_err(into_sql_error)?,
                seq: row.get::<_, i64>(3)? as u64,
                ts: parse_rfc3339(&row.get::<_, String>(4)?).map_err(into_sql_error)?,
                actor: row.get(5)?,
                event_type: row.get(6)?,
                payload_json: serde_json::from_str(&payload_json).map_err(into_sql_error)?,
                checksum: row.get(8)?,
                journal_path: row.get(9)?,
                journal_byte_offset: row.get::<_, i64>(10)? as u64,
                journal_line: row.get::<_, i64>(11)? as usize,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    pub fn increment_memory_access(&self, memory_ids: &[MemoryId]) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        for id in memory_ids {
            self.conn.execute(
                r#"
                UPDATE memory_objects
                SET access_count = access_count + 1, last_accessed_at = ?2
                WHERE memory_id = ?1
                "#,
                params![id.as_str(), now],
            )?;
        }
        Ok(())
    }

    pub fn update_memory_importance(
        &self,
        memory_id: &MemoryId,
        importance: f64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE memory_objects SET importance = ?2 WHERE memory_id = ?1",
            params![memory_id.as_str(), importance],
        )?;
        Ok(())
    }

    pub fn count_active_memories(
        &self,
        session_id: &SessionId,
        kind: Option<MemoryObjectKind>,
    ) -> Result<usize> {
        let count = if let Some(kind) = kind {
            self.conn.query_row(
                "SELECT COUNT(*) FROM memory_objects WHERE session_id = ?1 AND kind = ?2 AND status != 'superseded'",
                params![session_id.as_str(), kind.as_str()],
                |row| row.get::<_, i64>(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM memory_objects WHERE session_id = ?1 AND status != 'superseded'",
                params![session_id.as_str()],
                |row| row.get::<_, i64>(0),
            )?
        };
        Ok(count as usize)
    }

    pub fn session_exists(&self, session_id: &SessionId) -> Result<bool> {
        let count = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE session_id = ?1",
            params![session_id.as_str()],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }

    pub fn reconcile_session(&self, journal: &Journal, session_id: &SessionId) -> Result<()> {
        for replay in journal.replay()? {
            if &replay.event.session_id != session_id {
                continue;
            }
            self.record_event(
                &replay.event,
                &journal.path().display().to_string(),
                JournalAppend {
                    seq: replay.event.seq,
                    byte_offset: replay.byte_offset,
                    line_number: replay.line_number,
                },
            )?;
        }
        Ok(())
    }
}

const MEMORY_SELECT_COLUMNS: &str = r#"
    memory_id, session_id, kind, status, text, confidence, source_event_ids_json,
    created_at, superseded_by,
    source_adapter, source_model, importance, access_count, last_accessed_at,
    consolidated_from_json, text_hash
"#;

fn map_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryObject> {
    let source_event_ids_json: String = row.get(6)?;
    let source_event_ids =
        serde_json::from_str::<Vec<String>>(&source_event_ids_json).map_err(into_sql_error)?;
    let source_event_ids = source_event_ids
        .into_iter()
        .map(EventId::parse)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(into_sql_error)?;
    let kind = match row.get::<_, String>(2)?.as_str() {
        "fact" => MemoryObjectKind::Fact,
        "constraint" => MemoryObjectKind::Constraint,
        "decision" => MemoryObjectKind::Decision,
        "todo" => MemoryObjectKind::Todo,
        "summary" => MemoryObjectKind::Summary,
        "entity" => MemoryObjectKind::Entity,
        "file_note" => MemoryObjectKind::FileNote,
        other => {
            return Err(into_sql_error(ContynuError::Validation(format!(
                "unknown memory kind `{other}`"
            ))))
        }
    };

    let consolidated_from_json: Option<String> = row.get(14)?;
    let consolidated_from = consolidated_from_json
        .map(|json| {
            serde_json::from_str::<Vec<String>>(&json)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|s| MemoryId::parse(s).ok())
                .collect()
        })
        .unwrap_or_default();

    let last_accessed_at = row
        .get::<_, Option<String>>(13)?
        .map(|s| parse_rfc3339(&s))
        .transpose()
        .map_err(into_sql_error)?;

    Ok(MemoryObject {
        memory_id: MemoryId::parse(row.get::<_, String>(0)?).map_err(into_sql_error)?,
        session_id: SessionId::parse(row.get::<_, String>(1)?).map_err(into_sql_error)?,
        kind,
        status: row.get(3)?,
        text: row.get(4)?,
        confidence: row.get(5)?,
        source_event_ids,
        created_at: parse_rfc3339(&row.get::<_, String>(7)?).map_err(into_sql_error)?,
        superseded_by: row
            .get::<_, Option<String>>(8)?
            .map(MemoryId::parse)
            .transpose()
            .map_err(into_sql_error)?,
        source_adapter: row.get(9)?,
        source_model: row.get(10)?,
        importance: row.get::<_, Option<f64>>(11)?.unwrap_or(0.5),
        access_count: row.get::<_, Option<u32>>(12)?.unwrap_or(0),
        last_accessed_at,
        consolidated_from,
        text_hash: row.get(15)?,
    })
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .map_err(|error| ContynuError::Validation(error.to_string()))?
        .with_timezone(&Utc))
}

fn into_sql_error(error: impl ToString) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::{MemoryObjectKind, MetadataStore, SessionRecord};
    use crate::event::{Actor, EventDraft, EventType};
    use crate::ids::SessionId;
    use crate::journal::Journal;

    #[test]
    fn migrations_and_reconcile_work() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("contynu.db");
        let journal_path = dir.path().join("session.jsonl");
        let store = MetadataStore::open(&db).unwrap();
        let journal = Journal::open(&journal_path).unwrap();
        let session_id = SessionId::new();

        store
            .register_session(&SessionRecord {
                session_id: session_id.clone(),
                project_id: None,
                status: "started".into(),
                cli_name: None,
                cli_version: None,
                model_name: None,
                cwd: None,
                repo_root: None,
                host_fingerprint: None,
                started_at: chrono::Utc::now(),
                ended_at: None,
            })
            .unwrap();

        journal
            .append(EventDraft::new(
                session_id.clone(),
                None,
                Actor::Runtime,
                EventType::SessionStarted,
                json!({"cwd": "/tmp"}),
            ))
            .unwrap();

        store.reconcile_session(&journal, &session_id).unwrap();
        assert_eq!(store.list_events_for_session(&session_id).unwrap().len(), 1);
    }

    #[test]
    fn list_current_files_and_active_memory_filters_work() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("contynu.db");
        let store = MetadataStore::open(&db).unwrap();
        let session_id = SessionId::new();

        store
            .register_session(&SessionRecord {
                session_id: session_id.clone(),
                project_id: None,
                status: "started".into(),
                cli_name: None,
                cli_version: None,
                model_name: None,
                cwd: None,
                repo_root: None,
                host_fingerprint: None,
                started_at: chrono::Utc::now(),
                ended_at: None,
            })
            .unwrap();

        let first_memory = crate::store::MemoryObject {
            memory_id: crate::ids::MemoryId::new(),
            session_id: session_id.clone(),
            kind: MemoryObjectKind::Summary,
            status: "superseded".into(),
            text: "old summary".into(),
            confidence: Some(0.5),
            source_event_ids: Vec::new(),
            created_at: chrono::Utc::now(),
            superseded_by: None,
            source_adapter: None,
            source_model: None,
            importance: 0.5,
            access_count: 0,
            last_accessed_at: None,
            consolidated_from: Vec::new(),
            text_hash: None,
        };
        let active_memory = crate::store::MemoryObject {
            memory_id: crate::ids::MemoryId::new(),
            session_id: session_id.clone(),
            kind: MemoryObjectKind::Summary,
            status: "active".into(),
            text: "current summary".into(),
            confidence: Some(0.9),
            source_event_ids: Vec::new(),
            created_at: chrono::Utc::now(),
            superseded_by: None,
            source_adapter: None,
            source_model: None,
            importance: 0.5,
            access_count: 0,
            last_accessed_at: None,
            consolidated_from: Vec::new(),
            text_hash: None,
        };
        store.insert_memory_object(&first_memory).unwrap();
        store.insert_memory_object(&active_memory).unwrap();

        store
            .register_file(&crate::store::FileRecord {
                file_id: crate::ids::FileId::new(),
                session_id: session_id.clone(),
                workspace_relative_path: "src/main.rs".into(),
                kind: "source_text".into(),
                last_known_sha256: Some("sha256:one".into()),
                last_snapshot_event_id: None,
                last_diff_event_id: None,
                observed_at: chrono::Utc::now(),
                is_generated: false,
            })
            .unwrap();
        store
            .register_file(&crate::store::FileRecord {
                file_id: crate::ids::FileId::new(),
                session_id: session_id.clone(),
                workspace_relative_path: "src/main.rs".into(),
                kind: "source_deleted".into(),
                last_known_sha256: None,
                last_snapshot_event_id: None,
                last_diff_event_id: None,
                observed_at: chrono::Utc::now(),
                is_generated: false,
            })
            .unwrap();

        let current_files = store.list_current_files(&session_id).unwrap();
        assert_eq!(current_files.len(), 1);
        assert_eq!(current_files[0].workspace_relative_path, "src/main.rs");

        let memory_hits = store.search_memory("summary").unwrap();
        assert_eq!(memory_hits.len(), 1);
        assert_eq!(memory_hits[0].text, "current summary");
    }
}
