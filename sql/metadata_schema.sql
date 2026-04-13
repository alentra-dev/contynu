PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS sessions (
  session_id TEXT PRIMARY KEY,
  project_id TEXT,
  status TEXT NOT NULL,
  cli_name TEXT,
  cli_version TEXT,
  model_name TEXT,
  cwd TEXT,
  repo_root TEXT,
  host_fingerprint TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT
);

CREATE TABLE IF NOT EXISTS blobs (
  blob_id TEXT PRIMARY KEY,
  sha256 TEXT NOT NULL UNIQUE,
  size_bytes INTEGER NOT NULL,
  mime_type TEXT,
  storage_path TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS checkpoints (
  checkpoint_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  rehydration_sha256 TEXT,
  manifest_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS memory_objects (
  memory_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  scope TEXT NOT NULL DEFAULT 'project',
  status TEXT NOT NULL,
  text TEXT NOT NULL,
  importance REAL NOT NULL DEFAULT 0.5,
  reason TEXT,
  source_model TEXT,
  superseded_by TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT,
  access_count INTEGER DEFAULT 0,
  last_accessed_at TEXT,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS prompts (
  prompt_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  verbatim TEXT NOT NULL,
  interpretation TEXT,
  interpretation_confidence REAL,
  source_model TEXT,
  created_at TEXT NOT NULL,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_checkpoints_session_created_at ON checkpoints(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_memory_session_kind ON memory_objects(session_id, kind, created_at);
CREATE INDEX IF NOT EXISTS idx_memory_active_importance ON memory_objects(session_id, status, importance DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_memory_scope ON memory_objects(scope, status, importance DESC);
CREATE INDEX IF NOT EXISTS idx_prompts_session ON prompts(session_id, created_at DESC);

CREATE TABLE IF NOT EXISTS ingested_sources (
  source_path TEXT PRIMARY KEY,
  source_tool TEXT NOT NULL,
  ingested_at TEXT NOT NULL,
  memory_count INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS working_set_entries (
  session_id TEXT NOT NULL,
  memory_id TEXT NOT NULL,
  rank_score REAL NOT NULL DEFAULT 0,
  source_reason TEXT,
  refreshed_at TEXT NOT NULL,
  PRIMARY KEY (session_id, memory_id),
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE,
  FOREIGN KEY(memory_id) REFERENCES memory_objects(memory_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_working_set_session_rank
  ON working_set_entries(session_id, rank_score DESC, refreshed_at DESC);

CREATE TABLE IF NOT EXISTS packet_observations (
  observation_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  summary_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_packet_observations_session_created
  ON packet_observations(session_id, created_at DESC);

CREATE TABLE IF NOT EXISTS schema_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
