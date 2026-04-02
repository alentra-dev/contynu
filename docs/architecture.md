# Contynu Architecture Blueprint

## Purpose

Contynu is a model-agnostic persistent memory layer for LLM workflows. Its purpose is to capture, persist, index, and rehydrate the full working state of AI-assisted work so that sessions can survive crashes, resume seamlessly, and transfer across models without loss of meaningful context.

This document defines the production architecture for Contynu. It is intentionally optimized for durability, performance, auditability, and extensibility rather than short-term speed of implementation.

---

## Product Principles

1. **Durability first**
   Every meaningful event must be persisted before it is considered committed.

2. **Model agnostic by default**
   Contynu must not depend on any one vendor’s session semantics.

3. **Exact recall plus structured recall**
   The system must support exact replay and higher-level structured memory. Semantic retrieval is an optional later layer, not the source of truth.

4. **Local-first trust model**
   The canonical state must be able to live entirely on the user’s machine.

5. **Append-only truth**
   Raw history is immutable. Derived views are recomputed or updated from the canonical event log.

6. **Lean execution path**
   The hot path for capture and persistence must stay small, deterministic, and fast.

7. **Explicit rehydration**
   A resumed model should receive an authoritative state packet, not an ad hoc dump.

---

## System Overview

Contynu has five core subsystems:

1. **Capture Runtime**
   Intercepts user input, assistant output, tool activity, file activity, and artifacts.

2. **Canonical Event Store**
   Stores the immutable append-only journal of all session events.

3. **Derived Memory Engine**
   Builds structured memory products from the journal: summaries, decisions, constraints, artifacts, open loops, and file notes.

4. **Rehydration Engine**
   Produces a deterministic state packet for resume, handoff, or model switch.

5. **Adapter Layer**
   Normalizes integration with different CLIs, agents, and tool ecosystems through an explicit launcher config plus normalized runtime events.

---

## High-Level Architecture

```text
User ↔ Contynu Runtime Wrapper ↔ Target LLM CLI / Agent
                │
                ├── Event Journal (append-only)
                ├── Metadata Store (SQLite)
                ├── Blob Store (content-addressed)
                ├── Derived Memory Indexes
                └── Rehydration Packet Generator
```

The Contynu runtime sits between the user and the target LLM environment. It records the interaction stream, file/artifact changes, execution metadata, and rehydration context into durable local storage.

---

## Canonical Storage Design

### 1. Event Journal

The journal is the source of truth.

Properties:
- append-only
- ordered
- immutable
- checksummed
- session-aware
- replayable

Recommended format:
- newline-delimited JSON (`jsonl`) for the canonical raw stream
- one event per line
- monotonic sequence IDs per session

Example event categories:
- `session_started`
- `message_input`
- `message_output`
- `tool_call`
- `tool_result`
- `file_snapshot`
- `file_diff`
- `artifact_created`
- `artifact_read`
- `checkpoint_created`
- `session_interrupted`
- `session_resumed`

### 2. Metadata Store

A local SQLite database indexes and relates the journal content.

Responsibilities:
- session metadata
- turn metadata
- event pointers
- artifact registry
- file registry
- checkpoint registry
- retrieval indexes
- structured memory objects

SQLite is preferred for:
- low operational overhead
- transactional guarantees
- high enough performance for local-first single-user and small-team scenarios
- portability

### 3. Blob Store

Large or binary assets are stored separately in a content-addressed blob store.

Examples:
- uploaded PDFs
- generated DOCX/PDF/PPTX/XLSX files
- images
- binary outputs
- large file snapshots

Blobs are keyed by cryptographic digest.

---

## Data Model

### Session / Project
Represents the single continuous project memory timeline.

Fields:
- `session_id`
- `project_id`
- `started_at`
- `ended_at`
- `status`
- `cli_name`
- `cli_version`
- `model_name`
- `cwd`
- `repo_root`
- `host_fingerprint`

### Turn
Represents one command or interaction slice inside the long-lived project timeline.

Fields:
- `turn_id`
- `session_id`
- `started_at`
- `completed_at`
- `status`
- `summary_ref`

### Event
Represents one atomic captured action or state transition.

Fields:
- `event_id`
- `session_id`
- `turn_id`
- `seq`
- `ts`
- `actor`
- `event_type`
- `payload`
- `checksum`
- `parent_event_id`

### Artifact
Represents an external or generated file.

Fields:
- `artifact_id`
- `path`
- `mime_type`
- `sha256`
- `kind`
- `source_event_id`
- `created_at`

### Memory Object
Represents a derived reusable memory unit.

Kinds:
- `fact`
- `constraint`
- `decision`
- `todo`
- `summary`
- `entity`
- `file_note`

---

## Capture Runtime Design

### Runtime Role
The runtime is responsible for capturing and normalizing inputs from heterogeneous environments.

### Capture Sources
1. Standard input/output stream capture
2. In-process pseudo-terminal capture for interactive CLIs on Unix
3. Tool invocation capture where available
4. File scan and diff capture inside the working directory
5. Attachment and artifact capture
6. Environment and launcher metadata capture

### Design Requirement
The runtime must not block the target CLI unnecessarily. The hot path should:
1. normalize the event
2. durably append it
3. index it into SQLite
4. keep expensive enrichment off the append path

### Performance Goal
The persistence path must be optimized for small deterministic writes. Expensive enrichment belongs after the event is durably recorded.

---

## Rehydration Design

Rehydration is one of Contynu’s defining capabilities.

A fresh model or resumed process should not need the full raw transcript injected directly. Instead, it should receive an authoritative rehydration packet with references back to exact history.

### Rehydration Packet Sections
1. Mission / project purpose
2. Stable facts
3. Constraints and preferences
4. Decision log
5. Current state summary
6. Open loops / pending tasks
7. Relevant artifacts and files
8. Recent verbatim interaction window
9. Retrieval instructions / memory query interface

### Rehydration Modes
- **Resume**: continue the same session after interruption
- **Handoff**: move to a different model or provider
- **Review**: reconstruct state for human review
- **Checkpoint Restore**: recover to a known saved point

---

## Retrieval Design

Contynu should support three retrieval modes:

### 1. Exact Retrieval
Use when fidelity matters.
- by session ID
- by turn ID
- by event ID
- by file path
- by artifact ID
- by time range

### 2. Structured Retrieval
Use the derived memory store.
- decisions
- constraints
- open tasks
- summaries
- artifacts

### 3. Semantic Retrieval
Use embeddings and ranking for fuzzy recall.
- similar past problem
- related prior file changes
- relevant earlier discussion

The system must never rely on embeddings alone for authoritative recovery.

---

## Adapter Architecture

Adapters translate tool-specific behavior into Contynu’s normalized event model.

### Adapter Tiers
1. **Generic PTY Adapter**
   Works with most CLIs via terminal capture.

2. **Native Session Adapter**
   Ingests a tool’s own transcripts, logs, or checkpoints when available.

3. **Deep Integration Adapter**
   Uses vendor-specific APIs or hooks when justified.

### Adapter Contract
Each adapter must define:
- detection
- launch behavior
- input normalization
- output normalization
- session metadata extraction
- optional native import

---

## File and Artifact Strategy

Contynu must distinguish between:
- files that existed
- files the model saw
- files the model modified
- files produced as outputs

### Storage Strategy
- store metadata and hash for every relevant file reference
- store full content for small files
- store diff plus periodic snapshot for evolving text files
- store binary artifacts in blob store

### Ignore Policy
The system must support configurable ignore patterns for:
- build outputs
- caches
- virtual environments
- package directories
- generated noise

---

## Checkpoint Strategy

Checkpoints are lightweight deterministic snapshots of working state.

Trigger points:
- after a completed turn
- before risky operations
- before model handoff
- after substantial file changes
- at periodic intervals during long-running sessions

Checkpoint contents:
- checkpoint ID
- latest event sequence
- summary snapshot
- decision snapshot
- open tasks
- artifact registry slice
- recent files
- rehydration packet

---

## Security and Trust Model

### Baseline Requirements
- local-first by default
- no mandatory cloud dependency
- encryption at rest support
- secret redaction pipeline
- configurable retention policies
- per-project isolation

### Future Enterprise Controls
- role-based access controls
- audit export
- workspace policy engine
- managed sync
- central key management

---

## Recommended Implementation Stack

### Core Runtime Language
**Rust** is the preferred long-term implementation language for the capture runtime and hot path because of:
- performance
- correctness
- memory safety
- strong CLI/system programming support

### Practical Development Strategy
Use a **hybrid approach**:
- Rust for runtime, persistence hot path, file watcher, and adapter host
- Python only for optional analysis or experimental retrieval tooling if needed

### Storage
- JSONL journal for raw canonical log
- SQLite for metadata and structured memory
- content-addressed local blob store for artifacts

### Indexing
- start with deterministic metadata indexes
- add embedding index only after core exact/structured retrieval is stable

---

## Non-Goals for the Core

The following should not drive the initial architecture:
- hosted SaaS first
- multi-tenant cloud-first design
- heavy GUI before core durability is proven
- overfitting to one LLM vendor
- embedding-first memory without exact replay support

---

## Build Order

### Foundation Phase
1. Canonical event model
2. Journal writer and recovery logic
3. SQLite metadata schema
4. Blob store
5. Checkpoint format
6. Rehydration packet generator

### Runtime Phase
7. Generic PTY runtime wrapper
8. File watcher and snapshot/diff engine
9. Artifact capture
10. Structured memory derivation

### Integration Phase
11. Adapter SDK
12. Native adapters for major CLIs
13. Exact replay and search commands
14. Semantic retrieval layer

### Platform Phase
15. Team sync and governance
16. Hosted control plane
17. Enterprise policy controls

---

## Summary

Contynu should be built as a lean but serious systems product:
- append-only truth
- deterministic recovery
- model-agnostic integration
- local-first trust
- layered retrieval
- rehydration as a first-class primitive

That combination gives it both technical credibility and product defensibility.
