# Storage Model

## Purpose

This document defines the canonical storage architecture for Contynu, including structured metadata, blob handling, checkpoint materialization, and compatibility expectations.

Contynu's storage model is designed to satisfy five requirements simultaneously:
- durability
- inspectability
- performance
- portability
- deterministic recovery

---

## Storage Layers

Contynu uses three storage layers:

1. **Metadata Layer**
   SQLite-backed structured store for sessions, memories, prompts, and checkpoints.

2. **Blob Layer**
   Content-addressed storage for binary assets and large content.

3. **Checkpoint Layer**
   Materialized recovery bundles derived from metadata.

---

## 1. Metadata Layer

### Role
SQLite is the primary structured store and source of truth for all project state.

### Responsibilities
- session registry
- memory object registry (model-written)
- prompt registry (user prompts recorded verbatim)
- checkpoint registry
- blob metadata
- schema/version metadata

### Database Location
```text
.contynu/sqlite/contynu.db
```

### Core Tables (Schema v5)

#### `schema_meta`
Tracks database schema and compatibility state.

Columns:
- `key`
- `value`
- `updated_at`

#### `sessions`
Columns:
- `session_id`
- `project_id`
- `status`
- `started_at`
- `ended_at`
- `cli_name`
- `cli_version`
- `model_name`
- `cwd`
- `repo_root`
- `host_fingerprint`

#### `memory_objects`
Model-written structured memories.

Columns:
- `memory_id`
- `session_id`
- `kind` — fact, constraint, decision, todo, user_fact, project_knowledge
- `scope` — user, project, session
- `status` — active or superseded
- `text`
- `importance` — 0.0 to 1.0
- `reason` — why the memory is worth storing
- `source_model`
- `superseded_by`
- `created_at`
- `updated_at`
- `access_count`
- `last_accessed_at`

#### `prompts`
User prompts recorded verbatim.

Columns:
- `prompt_id`
- `session_id`
- `verbatim`
- `interpretation`
- `interpretation_confidence`
- `source_model`
- `created_at`

#### `checkpoints`
Columns:
- `checkpoint_id`
- `session_id`
- `reason`
- `rehydration_sha256`
- `manifest_json`
- `created_at`

#### `blobs`
Columns:
- `blob_id`
- `sha256`
- `size_bytes`
- `mime_type`
- `storage_path`
- `created_at`

### Indexing

- `memory_objects(session_id, kind, created_at)`
- `memory_objects(session_id, status, importance DESC, created_at DESC)`
- `memory_objects(scope, status, importance DESC)`
- `prompts(session_id, created_at DESC)`
- `checkpoints(session_id, created_at)`

---

## 2. Blob Layer

### Role
Stores large or binary content outside SQLite.

### Use Cases
- rehydration packets
- uploaded files
- generated outputs

### Addressing Model
Content-addressed by SHA-256 digest.

### File Layout
```text
.contynu/blobs/sha256/ab/cd/abcdef...
```

### Deduplication Rule
If a blob with the same digest already exists, the store reuses it rather than writing duplicate content.

---

## 3. Checkpoint Layer

### Role
Materialized recovery artifacts for resume and handoff.

### File Layout
```text
.contynu/checkpoints/
  prj_<id>/
    chk_<id>/
      manifest.json
      rehydration.json
```

### Contents
A checkpoint includes:
- checkpoint manifest
- memory snapshot
- rehydration payload

### Checkpoint Principle
A checkpoint is a derived materialization from the metadata layer.

---

## Overall File Layout

```text
.contynu/
  sqlite/
    contynu.db
  blobs/
    sha256/ab/cd/<digest>
  checkpoints/
    prj_<id>/
      chk_<id>/
        manifest.json
        rehydration.json
  config.json
```

---

## Versioning and Compatibility

### SQLite Schema Compatibility
- managed via explicit migrations
- migration history tracked in `schema_meta`
- current schema version: v5

### Blob Compatibility
- blob storage is content-addressed and largely schema-independent
- metadata changes must not invalidate existing blob references

### Checkpoint Compatibility
- checkpoints may become stale when newer rehydration logic is introduced
- old checkpoints may be re-materialized from metadata rather than migrated directly

---

## Legacy Data Cleanup

On startup, Contynu detects and removes legacy v0.4.0 storage artifacts:
- `journal/` directory and JSONL files
- `runtime/` directory
- Legacy DB tables: events, turns, files, artifacts

This ensures a clean transition to the model-driven architecture.

---

## Durability Guarantees

1. SQLite WAL mode ensures crash safety for metadata writes.
2. Blob writes are verified by digest before registration.
3. Recovery never silently skips corrupt data.

---

## Summary

Contynu's storage model is intentionally simple:
- SQLite for structured state (memories, prompts, sessions, checkpoints)
- blob store for heavy content
- checkpoints for derived recovery bundles

This provides strong recovery semantics while staying lean, transparent, and local-first.
