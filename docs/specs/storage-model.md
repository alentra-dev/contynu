# Storage Model

## Purpose

This document defines the canonical storage architecture for Contynu, including raw event persistence, structured metadata, blob handling, checkpoint materialization, and compatibility expectations.

Contynu’s storage model is designed to satisfy five requirements simultaneously:
- durability
- inspectability
- performance
- portability
- deterministic recovery

---

## Storage Layers

Contynu uses four storage layers:

1. **Journal Layer**
   Immutable append-only event log.

2. **Metadata Layer**
   SQLite-backed relational index and structured memory registry.

3. **Blob Layer**
   Content-addressed storage for binary assets and large text snapshots.

4. **Checkpoint Layer**
   Materialized recovery bundles derived from journal + metadata.

These layers have different roles and must not be collapsed into one opaque store.

---

## 1. Journal Layer

### Role
The journal is the canonical source of truth.

### Format
- newline-delimited JSON (`.jsonl`)
- one canonical event per line
- append-only
- durable write semantics

### File Layout
Recommended root path:

```text
.contynu/
  journal/
    2026/
      04/
        03/
          ses_01J...jsonl
```

Alternative segmenting strategies may be supported later, but the initial layout should remain simple and debuggable.

### Write Rules
- sequence number assigned at commit time
- checksum computed after final canonical serialization
- fsync or equivalent durability boundary before acknowledging commit success
- partial trailing lines after crash must be detected and handled safely during recovery

### Recovery Rules
On recovery:
1. scan journal tail
2. detect truncated or corrupt trailing entry
3. preserve valid prefix
4. record recovery event if repair occurred

### Journal Invariants
- immutable once committed
- per-session sequence strictly increasing
- checksummed
- replay order determined by `seq`

---

## 2. Metadata Layer

### Role
SQLite stores structured state derived from or linked to the journal.

### Responsibilities
- session registry
- turn registry
- event index
- file registry
- artifact registry
- checkpoint registry
- memory object registry
- retrieval indexes
- schema/version metadata

### Database Location
```text
.contynu/sqlite/contynu.db
```

### Core Tables

#### `schema_meta`
Tracks database schema and compatibility state.

Suggested columns:
- `key`
- `value`
- `updated_at`

#### `sessions`
Suggested columns:
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

#### `turns`
Suggested columns:
- `turn_id`
- `session_id`
- `status`
- `started_at`
- `completed_at`
- `summary_memory_id`

#### `events`
Suggested columns:
- `event_id`
- `session_id`
- `turn_id`
- `seq`
- `ts`
- `actor`
- `event_type`
- `payload_version`
- `journal_path`
- `journal_byte_offset`
- `checksum`
- `correlation_id`
- `causation_id`

#### `artifacts`
Suggested columns:
- `artifact_id`
- `session_id`
- `path`
- `kind`
- `mime_type`
- `sha256`
- `blob_id`
- `created_at`
- `deleted_at`

#### `files`
Suggested columns:
- `file_id`
- `session_id`
- `workspace_relative_path`
- `last_known_sha256`
- `last_snapshot_event_id`
- `last_diff_event_id`
- `observed_at`

#### `checkpoints`
Suggested columns:
- `checkpoint_id`
- `session_id`
- `created_at`
- `reason`
- `last_seq`
- `rehydration_blob_id`

#### `memory_objects`
Suggested columns:
- `memory_id`
- `session_id`
- `kind`
- `status`
- `text`
- `confidence`
- `source_event_ids_json`
- `created_at`
- `superseded_by`

### Indexing Guidance
At minimum:
- `events(session_id, seq)` unique
- `events(event_id)` unique
- `turns(session_id, started_at)`
- `artifacts(session_id, created_at)`
- `memory_objects(session_id, kind, created_at)`

---

## 3. Blob Layer

### Role
Stores large or binary content outside SQLite and the raw journal.

### Use Cases
- uploaded files
- generated outputs
- file snapshots
- diffs
- terminal output captures beyond inline threshold
- rehydration packets

### Addressing Model
Content-addressed by digest.

Recommended primary key:
- `sha256`

Optional internal blob ID may also be maintained.

### File Layout
```text
.contynu/blobs/sha256/ab/cd/abcdef...
```

### Blob Metadata
Blob metadata may be stored in SQLite with fields such as:
- `blob_id`
- `sha256`
- `size_bytes`
- `mime_type`
- `storage_path`
- `created_at`

### Deduplication Rule
If a blob with the same digest already exists, the store must reuse it rather than writing duplicate content.

---

## 4. Checkpoint Layer

### Role
Materialized recovery artifacts for resume and handoff.

### File Layout
```text
.contynu/checkpoints/
  ses_01J.../
    chk_01J.../
      manifest.json
      rehydration.json
      summary.md
```

### Contents
A checkpoint should include:
- checkpoint manifest
- last committed event sequence
- current state summary
- decision and constraint slices
- open loops
- relevant artifact references
- recent context window
- deterministic rehydration payload

### Checkpoint Principle
A checkpoint is not a new source of truth. It is a derived materialization of truth from the journal and metadata layers.

---

## Inline vs Blob Thresholds

To keep the journal lean, payloads may inline small content and externalize large content.

Recommended rule:
- small structured text may be inlined directly in event payload
- large text or binary data should move to blob store and be referenced by ID

Threshold values should be configurable, but the default should favor journal readability without bloating it.

---

## Versioning and Compatibility

### Journal Compatibility
- journal entries are governed by `schema_version` and `payload_version`
- readers must preserve compatibility with prior supported versions

### SQLite Schema Compatibility
- managed via explicit migrations
- migration history must be tracked in `schema_meta`

### Blob Compatibility
- blob storage is content-addressed and largely schema-independent
- metadata changes must not invalidate existing blob references

### Checkpoint Compatibility
- checkpoints may become stale when newer rehydration logic is introduced
- old checkpoints may be re-materialized from journal + metadata rather than migrated directly

---

## Durability Guarantees

The system should provide these guarantees:

1. If an event commit returns success, it must survive process crash.
2. Metadata updates associated with a committed event must be recoverable to a consistent state.
3. Recovery must never silently skip corrupt data; it must either repair deterministically or surface the fault.
4. Blob writes must be verified by digest before registration.

---

## Failure Model

### Possible Failures
- process crash during journal append
- crash between journal append and metadata update
- partial blob write
- checkpoint generation interruption
- disk full or I/O error

### Recovery Strategy
- journal is repaired first
- metadata is reconciled from journal if needed
- orphan blobs may be garbage-collected later
- incomplete checkpoints are discarded or rebuilt

---

## Garbage Collection

Initial Contynu should favor correctness over aggressive cleanup.

Safe cleanup candidates:
- orphan temporary files
- incomplete checkpoint directories
- unreferenced blobs after reconciliation

Canonical journal events must never be garbage-collected by default.

---

## Summary

Contynu’s storage model is intentionally layered:
- journal for immutable truth
- SQLite for queryable structure
- blob store for heavy content
- checkpoints for derived recovery bundles

This provides strong recovery semantics while staying lean, transparent, and local-first.
