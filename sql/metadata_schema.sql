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

CREATE TABLE IF NOT EXISTS turns (
  turn_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  status TEXT NOT NULL,
  started_at TEXT NOT NULL,
  completed_at TEXT,
  summary_memory_id TEXT,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS events (
  event_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  turn_id TEXT,
  seq INTEGER NOT NULL,
  ts TEXT NOT NULL,
  actor TEXT NOT NULL,
  event_type TEXT NOT NULL,
  payload_version INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  checksum TEXT NOT NULL,
  parent_event_id TEXT,
  correlation_id TEXT,
  causation_id TEXT,
  tags_json TEXT NOT NULL,
  journal_path TEXT NOT NULL,
  journal_byte_offset INTEGER NOT NULL,
  journal_line INTEGER NOT NULL,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE,
  FOREIGN KEY(turn_id) REFERENCES turns(turn_id) ON DELETE SET NULL,
  FOREIGN KEY(parent_event_id) REFERENCES events(event_id) ON DELETE SET NULL,
  FOREIGN KEY(causation_id) REFERENCES events(event_id) ON DELETE SET NULL,
  UNIQUE(session_id, seq)
);

CREATE TABLE IF NOT EXISTS blobs (
  blob_id TEXT PRIMARY KEY,
  sha256 TEXT NOT NULL UNIQUE,
  size_bytes INTEGER NOT NULL,
  mime_type TEXT,
  storage_path TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artifacts (
  artifact_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  source_event_id TEXT NOT NULL,
  path TEXT,
  kind TEXT NOT NULL,
  mime_type TEXT,
  sha256 TEXT NOT NULL,
  blob_relative_path TEXT NOT NULL,
  size_bytes INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE,
  FOREIGN KEY(source_event_id) REFERENCES events(event_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS files (
  file_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  workspace_relative_path TEXT NOT NULL,
  kind TEXT NOT NULL,
  last_known_sha256 TEXT,
  last_snapshot_event_id TEXT,
  last_diff_event_id TEXT,
  observed_at TEXT NOT NULL,
  is_generated INTEGER NOT NULL DEFAULT 0,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE,
  FOREIGN KEY(last_snapshot_event_id) REFERENCES events(event_id) ON DELETE SET NULL,
  FOREIGN KEY(last_diff_event_id) REFERENCES events(event_id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS checkpoints (
  checkpoint_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  source_event_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  last_seq INTEGER NOT NULL,
  rehydration_sha256 TEXT,
  manifest_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE,
  FOREIGN KEY(source_event_id) REFERENCES events(event_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS memory_objects (
  memory_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  text TEXT NOT NULL,
  confidence REAL,
  source_event_ids_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  superseded_by TEXT,
  FOREIGN KEY(session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_turns_session_started_at ON turns(session_id, started_at);
CREATE INDEX IF NOT EXISTS idx_events_session_seq ON events(session_id, seq);
CREATE INDEX IF NOT EXISTS idx_events_event_id ON events(event_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_session_created_at ON artifacts(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_files_session_path ON files(session_id, workspace_relative_path);
CREATE INDEX IF NOT EXISTS idx_checkpoints_session_created_at ON checkpoints(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_memory_session_kind_created_at ON memory_objects(session_id, kind, created_at);
