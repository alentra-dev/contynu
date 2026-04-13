use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::blobs::BlobDescriptor;
use crate::checkpoint::CheckpointManifest;
use crate::error::{ContynuError, Result};
use crate::ids::{CheckpointId, MemoryId, SessionId};

const MIGRATION_V5: &str = include_str!("../../../sql/metadata_schema.sql");

// ---------------------------------------------------------------------------
// Record types
// ---------------------------------------------------------------------------

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
pub struct CheckpointRecord {
    pub checkpoint_id: CheckpointId,
    pub session_id: SessionId,
    pub reason: String,
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
    UserFact,
    ProjectKnowledge,
}

impl MemoryObjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Constraint => "constraint",
            Self::Decision => "decision",
            Self::Todo => "todo",
            Self::UserFact => "user_fact",
            Self::ProjectKnowledge => "project_knowledge",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fact" => Some(Self::Fact),
            "constraint" => Some(Self::Constraint),
            "decision" => Some(Self::Decision),
            "todo" => Some(Self::Todo),
            "user_fact" => Some(Self::UserFact),
            "project_knowledge" => Some(Self::ProjectKnowledge),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    User,
    Project,
    Session,
}

impl MemoryScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Session => "session",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "project" => Some(Self::Project),
            "session" => Some(Self::Session),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryObject {
    pub memory_id: MemoryId,
    pub session_id: SessionId,
    pub kind: MemoryObjectKind,
    pub scope: MemoryScope,
    pub status: String,
    pub text: String,
    pub importance: f64,
    pub reason: Option<String>,
    pub source_model: Option<String>,
    pub superseded_by: Option<MemoryId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub access_count: u32,
    pub last_accessed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRecord {
    pub prompt_id: String,
    pub session_id: SessionId,
    pub verbatim: String,
    pub interpretation: Option<String>,
    pub interpretation_confidence: Option<f64>,
    pub source_model: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Query parameters for flexible memory search.
pub struct MemoryQuery {
    pub session_id: Option<SessionId>,
    pub text_query: Option<String>,
    pub kind: Option<MemoryObjectKind>,
    pub scope: Option<MemoryScope>,
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
            scope: None,
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

// ---------------------------------------------------------------------------
// MetadataStore
// ---------------------------------------------------------------------------

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

    pub fn open_readwrite(path: impl AsRef<Path>) -> Result<Self> {
        Self::open(path)
    }

    pub fn migrate(&self) -> Result<()> {
        // Check if we need a full reset (v5 architecture) or fresh install
        let has_schema_meta = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_meta'",
            [],
            |row| row.get::<_, i64>(0),
        ).unwrap_or(0) > 0;

        let current_version = if has_schema_meta {
            self.conn.query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            ).optional()?.and_then(|v| v.parse::<i64>().ok()).unwrap_or(0)
        } else {
            0
        };

        if current_version >= 5 {
            return Ok(());
        }

        // If upgrading from old architecture (v1-v4), drop legacy tables
        if current_version > 0 && current_version < 5 {
            self.conn.execute_batch(
                r#"
                DROP TABLE IF EXISTS events;
                DROP TABLE IF EXISTS turns;
                DROP TABLE IF EXISTS files;
                DROP TABLE IF EXISTS artifacts;
                DROP TABLE IF EXISTS checkpoints;
                DROP TABLE IF EXISTS memory_objects;
                DROP TABLE IF EXISTS blobs;
                DROP TABLE IF EXISTS schema_migrations;
                DROP TABLE IF EXISTS schema_meta;
                DROP TABLE IF EXISTS prompts;
                "#,
            )?;
        }

        // Apply v5 schema (fresh)
        self.conn.execute_batch(MIGRATION_V5)?;

        self.conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params!["schema_version", "5", Utc::now().to_rfc3339()],
        )?;

        Ok(())
    }

    // -- Session operations ---------------------------------------------------

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
                session.ended_at.map(|v| v.to_rfc3339())
            ],
        )?;
        Ok(())
    }

    pub fn primary_project_id(&self) -> Result<Option<SessionId>> {
        let value = self.conn.query_row(
            "SELECT value FROM schema_meta WHERE key = ?1",
            params![PRIMARY_PROJECT_KEY],
            |row| row.get::<_, String>(0),
        ).optional()?;
        value.map(SessionId::parse).transpose()
    }

    pub fn set_primary_project_id(&self, session_id: &SessionId) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value, updated_at) VALUES (?1, ?2, ?3)",
            params![PRIMARY_PROJECT_KEY, session_id.as_str(), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn get_session(&self, session_id: &SessionId) -> Result<Option<SessionRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT session_id, project_id, status, cli_name, cli_version, model_name, cwd,
                   repo_root, host_fingerprint, started_at, ended_at
            FROM sessions WHERE session_id = ?1
            "#,
        )?;
        let session = stmt.query_row(params![session_id.as_str()], |row| {
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
                ended_at: row.get::<_, Option<String>>(10)?
                    .map(|v| parse_rfc3339(&v)).transpose().map_err(into_sql_error)?,
            })
        }).optional()?;
        Ok(session)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT session_id, project_id, status, cli_name, cli_version, model_name, cwd,
                   repo_root, host_fingerprint, started_at, ended_at
            FROM sessions ORDER BY started_at DESC
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
                ended_at: row.get::<_, Option<String>>(10)?
                    .map(|v| parse_rfc3339(&v)).transpose().map_err(into_sql_error)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_session_status(
        &self, session_id: &SessionId, status: &str, ended_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status = ?2, ended_at = ?3 WHERE session_id = ?1",
            params![session_id.as_str(), status, ended_at.map(|v| v.to_rfc3339())],
        )?;
        Ok(())
    }

    pub fn session_exists(&self, session_id: &SessionId) -> Result<bool> {
        let count = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE session_id = ?1",
            params![session_id.as_str()],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }

    // -- Blob operations ------------------------------------------------------

    pub fn register_blob(&self, blob: &BlobDescriptor, mime_type: Option<&str>) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO blobs (blob_id, sha256, size_bytes, mime_type, storage_path, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![blob.sha256, blob.sha256, blob.size_bytes as i64, mime_type, blob.relative_path, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    // -- Checkpoint operations ------------------------------------------------

    pub fn register_checkpoint(
        &self, checkpoint: &CheckpointRecord, manifest: &CheckpointManifest,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO checkpoints (
              checkpoint_id, session_id, reason, rehydration_sha256, manifest_json, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                checkpoint.checkpoint_id.as_str(),
                checkpoint.session_id.as_str(),
                checkpoint.reason,
                checkpoint.rehydration_sha256,
                serde_json::to_string(manifest)?,
                checkpoint.created_at.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    // -- Memory operations (model-driven) -------------------------------------

    pub fn insert_memory_object(&self, memory: &MemoryObject) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO memory_objects (
              memory_id, session_id, kind, scope, status, text, importance, reason,
              source_model, superseded_by, created_at, updated_at,
              access_count, last_accessed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            "#,
            params![
                memory.memory_id.as_str(),
                memory.session_id.as_str(),
                memory.kind.as_str(),
                memory.scope.as_str(),
                memory.status,
                memory.text,
                memory.importance,
                memory.reason,
                memory.source_model,
                memory.superseded_by.as_ref().map(|id| id.to_string()),
                memory.created_at.to_rfc3339(),
                memory.updated_at.map(|dt| dt.to_rfc3339()),
                memory.access_count,
                memory.last_accessed_at.map(|dt| dt.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    pub fn update_memory_text(
        &self, memory_id: &MemoryId, text: &str, importance: f64, reason: Option<&str>,
    ) -> Result<()> {
        let rows = self.conn.execute(
            r#"
            UPDATE memory_objects
            SET text = ?2, importance = ?3, reason = ?4, updated_at = ?5
            WHERE memory_id = ?1 AND status = 'active'
            "#,
            params![memory_id.as_str(), text, importance, reason, Utc::now().to_rfc3339()],
        )?;
        if rows == 0 {
            return Err(ContynuError::MemoryNotFound(memory_id.to_string()));
        }
        Ok(())
    }

    pub fn delete_memory(&self, memory_id: &MemoryId) -> Result<()> {
        let rows = self.conn.execute(
            "UPDATE memory_objects SET status = 'deleted' WHERE memory_id = ?1 AND status = 'active'",
            params![memory_id.as_str()],
        )?;
        if rows == 0 {
            return Err(ContynuError::MemoryNotFound(memory_id.to_string()));
        }
        Ok(())
    }

    pub fn supersede_memory(
        &self, memory_id: &MemoryId, superseded_by: &MemoryId,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            UPDATE memory_objects
            SET status = 'superseded', superseded_by = ?2
            WHERE memory_id = ?1 AND status = 'active'
            "#,
            params![memory_id.as_str(), superseded_by.as_str()],
        )?;
        Ok(())
    }

    pub fn get_memory(&self, memory_id: &MemoryId) -> Result<Option<MemoryObject>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MEMORY_COLUMNS} FROM memory_objects WHERE memory_id = ?1"
        ))?;
        let memory = stmt.query_row(params![memory_id.as_str()], map_memory).optional()?;
        Ok(memory)
    }

    pub fn list_active_memories(
        &self, session_id: &SessionId, kind: Option<MemoryObjectKind>,
    ) -> Result<Vec<MemoryObject>> {
        let sql = if kind.is_some() {
            format!(
                "SELECT {MEMORY_COLUMNS} FROM memory_objects
                 WHERE session_id = ?1 AND kind = ?2 AND status = 'active'
                 ORDER BY importance DESC, created_at DESC"
            )
        } else {
            format!(
                "SELECT {MEMORY_COLUMNS} FROM memory_objects
                 WHERE session_id = ?1 AND status = 'active'
                 ORDER BY importance DESC, created_at DESC"
            )
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = if let Some(kind) = kind {
            stmt.query_map(params![session_id.as_str(), kind.as_str()], map_memory)?
        } else {
            stmt.query_map(params![session_id.as_str()], map_memory)?
        };
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn query_memories(&self, query: &MemoryQuery) -> Result<Vec<MemoryObject>> {
        let mut conditions = vec!["status = 'active'".to_string()];
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
        if let Some(ref scope) = query.scope {
            conditions.push(format!("scope = ?{param_idx}"));
            param_values.push(Box::new(scope.as_str().to_string()));
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

        let sql = format!(
            "SELECT {MEMORY_COLUMNS} FROM memory_objects WHERE {} ORDER BY {} LIMIT {} OFFSET {}",
            conditions.join(" AND "), order, limit, query.offset
        );

        let params: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params.as_slice(), map_memory)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn search_memory(&self, query: &str) -> Result<Vec<MemoryObject>> {
        let needle = format!("%{query}%");
        let sql = format!(
            "SELECT {MEMORY_COLUMNS} FROM memory_objects
             WHERE text LIKE ?1 AND status = 'active'
             ORDER BY importance DESC, created_at DESC
             LIMIT 50"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![needle], map_memory)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn count_active_memories(
        &self, session_id: &SessionId, kind: Option<MemoryObjectKind>,
    ) -> Result<usize> {
        let count = if let Some(kind) = kind {
            self.conn.query_row(
                "SELECT COUNT(*) FROM memory_objects WHERE session_id = ?1 AND kind = ?2 AND status = 'active'",
                params![session_id.as_str(), kind.as_str()],
                |row| row.get::<_, i64>(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM memory_objects WHERE session_id = ?1 AND status = 'active'",
                params![session_id.as_str()],
                |row| row.get::<_, i64>(0),
            )?
        };
        Ok(count as usize)
    }

    pub fn increment_memory_access(&self, memory_ids: &[MemoryId]) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        for id in memory_ids {
            self.conn.execute(
                "UPDATE memory_objects SET access_count = access_count + 1, last_accessed_at = ?2 WHERE memory_id = ?1",
                params![id.as_str(), now],
            )?;
        }
        Ok(())
    }

    // -- Prompt operations ----------------------------------------------------

    pub fn insert_prompt(&self, prompt: &PromptRecord) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO prompts (
              prompt_id, session_id, verbatim, interpretation, interpretation_confidence,
              source_model, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                prompt.prompt_id,
                prompt.session_id.as_str(),
                prompt.verbatim,
                prompt.interpretation,
                prompt.interpretation_confidence,
                prompt.source_model,
                prompt.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list_recent_prompts(&self, session_id: &SessionId, limit: usize) -> Result<Vec<PromptRecord>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT prompt_id, session_id, verbatim, interpretation, interpretation_confidence,
                   source_model, created_at
            FROM prompts WHERE session_id = ?1
            ORDER BY created_at DESC LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![session_id.as_str(), limit as i64], |row| {
            Ok(PromptRecord {
                prompt_id: row.get(0)?,
                session_id: SessionId::parse(row.get::<_, String>(1)?).map_err(into_sql_error)?,
                verbatim: row.get(2)?,
                interpretation: row.get(3)?,
                interpretation_confidence: row.get(4)?,
                source_model: row.get(5)?,
                created_at: parse_rfc3339(&row.get::<_, String>(6)?).map_err(into_sql_error)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Purge all data from old architecture (events, turns, files, etc.)
    /// Called on startup when migrating to v5.
    pub fn purge_old_data(&self) -> Result<()> {
        // Drop old tables if they exist (idempotent)
        self.conn.execute_batch(
            r#"
            DROP TABLE IF EXISTS events;
            DROP TABLE IF EXISTS turns;
            DROP TABLE IF EXISTS files;
            DROP TABLE IF EXISTS artifacts;
            "#,
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const MEMORY_COLUMNS: &str = r#"
    memory_id, session_id, kind, scope, status, text, importance, reason,
    source_model, superseded_by, created_at, updated_at,
    access_count, last_accessed_at
"#;

fn map_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryObject> {
    let kind = MemoryObjectKind::from_str(&row.get::<_, String>(2)?)
        .ok_or_else(|| into_sql_error(ContynuError::Validation("unknown memory kind".into())))?;
    let scope = MemoryScope::from_str(&row.get::<_, String>(3)?)
        .ok_or_else(|| into_sql_error(ContynuError::Validation("unknown memory scope".into())))?;

    Ok(MemoryObject {
        memory_id: MemoryId::parse(row.get::<_, String>(0)?).map_err(into_sql_error)?,
        session_id: SessionId::parse(row.get::<_, String>(1)?).map_err(into_sql_error)?,
        kind,
        scope,
        status: row.get(4)?,
        text: row.get(5)?,
        importance: row.get::<_, Option<f64>>(6)?.unwrap_or(0.5),
        reason: row.get(7)?,
        source_model: row.get(8)?,
        superseded_by: row.get::<_, Option<String>>(9)?
            .map(MemoryId::parse).transpose().map_err(into_sql_error)?,
        created_at: parse_rfc3339(&row.get::<_, String>(10)?).map_err(into_sql_error)?,
        updated_at: row.get::<_, Option<String>>(11)?
            .map(|s| parse_rfc3339(&s)).transpose().map_err(into_sql_error)?,
        access_count: row.get::<_, Option<u32>>(12)?.unwrap_or(0),
        last_accessed_at: row.get::<_, Option<String>>(13)?
            .map(|s| parse_rfc3339(&s)).transpose().map_err(into_sql_error)?,
    })
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .map_err(|e| ContynuError::Validation(e.to_string()))?
        .with_timezone(&Utc))
}

fn into_sql_error(error: impl ToString) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())),
    )
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use super::*;

    #[test]
    fn fresh_migration_works() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("contynu.db");
        let store = MetadataStore::open(&db).unwrap();
        let session_id = SessionId::new();
        store.register_session(&SessionRecord {
            session_id: session_id.clone(),
            project_id: None,
            status: "active".into(),
            cli_name: Some("claude_cli".into()),
            cli_version: None,
            model_name: None,
            cwd: None,
            repo_root: None,
            host_fingerprint: None,
            started_at: chrono::Utc::now(),
            ended_at: None,
        }).unwrap();
        store.set_primary_project_id(&session_id).unwrap();
        assert_eq!(store.primary_project_id().unwrap().unwrap(), session_id);
    }

    #[test]
    fn memory_crud_works() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("contynu.db");
        let store = MetadataStore::open(&db).unwrap();
        let session_id = SessionId::new();
        store.register_session(&SessionRecord {
            session_id: session_id.clone(),
            project_id: None,
            status: "active".into(),
            cli_name: None, cli_version: None, model_name: None,
            cwd: None, repo_root: None, host_fingerprint: None,
            started_at: chrono::Utc::now(), ended_at: None,
        }).unwrap();

        let mem_id = MemoryId::new();
        store.insert_memory_object(&MemoryObject {
            memory_id: mem_id.clone(),
            session_id: session_id.clone(),
            kind: MemoryObjectKind::Fact,
            scope: MemoryScope::Project,
            status: "active".into(),
            text: "The API uses JWT auth".into(),
            importance: 0.8,
            reason: Some("Model observed this".into()),
            source_model: Some("claude-opus-4-6".into()),
            superseded_by: None,
            created_at: chrono::Utc::now(),
            updated_at: None,
            access_count: 0,
            last_accessed_at: None,
        }).unwrap();

        let memories = store.list_active_memories(&session_id, None).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].text, "The API uses JWT auth");

        // Update
        store.update_memory_text(&mem_id, "The API uses OAuth2", 0.9, Some("Corrected")).unwrap();
        let updated = store.get_memory(&mem_id).unwrap().unwrap();
        assert_eq!(updated.text, "The API uses OAuth2");
        assert_eq!(updated.importance, 0.9);

        // Delete
        store.delete_memory(&mem_id).unwrap();
        let memories = store.list_active_memories(&session_id, None).unwrap();
        assert_eq!(memories.len(), 0);
    }

    #[test]
    fn prompt_recording_works() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("contynu.db");
        let store = MetadataStore::open(&db).unwrap();
        let session_id = SessionId::new();
        store.register_session(&SessionRecord {
            session_id: session_id.clone(),
            project_id: None, status: "active".into(),
            cli_name: None, cli_version: None, model_name: None,
            cwd: None, repo_root: None, host_fingerprint: None,
            started_at: chrono::Utc::now(), ended_at: None,
        }).unwrap();

        store.insert_prompt(&PromptRecord {
            prompt_id: "pmt_test1".into(),
            session_id: session_id.clone(),
            verbatim: "proceed".into(),
            interpretation: Some("Continue with Bug 2 reproduction".into()),
            interpretation_confidence: Some(0.9),
            source_model: Some("claude-opus-4-6".into()),
            created_at: chrono::Utc::now(),
        }).unwrap();

        let prompts = store.list_recent_prompts(&session_id, 10).unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].verbatim, "proceed");
    }

    #[test]
    fn memory_search_and_query_work() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("contynu.db");
        let store = MetadataStore::open(&db).unwrap();
        let session_id = SessionId::new();
        store.register_session(&SessionRecord {
            session_id: session_id.clone(),
            project_id: None, status: "active".into(),
            cli_name: None, cli_version: None, model_name: None,
            cwd: None, repo_root: None, host_fingerprint: None,
            started_at: chrono::Utc::now(), ended_at: None,
        }).unwrap();

        for (kind, scope, text, importance) in [
            (MemoryObjectKind::Fact, MemoryScope::Project, "JWT authentication is used", 0.8),
            (MemoryObjectKind::UserFact, MemoryScope::User, "Udonna created Contynu", 0.9),
            (MemoryObjectKind::Constraint, MemoryScope::Project, "Must support backward compat", 0.95),
        ] {
            store.insert_memory_object(&MemoryObject {
                memory_id: MemoryId::new(),
                session_id: session_id.clone(),
                kind, scope, status: "active".into(),
                text: text.into(), importance,
                reason: None, source_model: None, superseded_by: None,
                created_at: chrono::Utc::now(), updated_at: None,
                access_count: 0, last_accessed_at: None,
            }).unwrap();
        }

        // Search by text
        let results = store.search_memory("JWT").unwrap();
        assert_eq!(results.len(), 1);

        // Query by scope
        let results = store.query_memories(&MemoryQuery {
            session_id: Some(session_id.clone()),
            scope: Some(MemoryScope::User),
            ..Default::default()
        }).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].text.contains("Udonna"));
    }
}
