# ADR 0001: Core Runtime and Storage Architecture

## Status
Accepted

## Context

Contynu is intended to be a high-trust, model-agnostic persistent memory layer for LLM workflows. The system must capture terminal interactions, file activity, artifacts, and execution metadata, then support deterministic recovery and cross-model handoff.

The core architectural choices must prioritize:
- durability
- performance
- auditability
- local-first operation
- low operational complexity
- vendor neutrality

The main design decision is the shape of the runtime and the canonical storage architecture.

---

## Decision

Contynu will use the following foundational architecture:

### 1. Runtime
The primary runtime will be implemented in **Rust**.

Rust is selected for the core runtime because it offers:
- predictable performance
- strong correctness guarantees
- safe systems-level programming
- excellent support for CLI, PTY, filesystem, and concurrency primitives

### 2. Canonical Event Store
The canonical history will be stored as an **append-only JSONL journal**.

Rationale:
- simple and inspectable
- durable and replayable
- easy to debug and export
- stable source of truth independent of internal database evolution

### 3. Metadata and Structured Memory
Structured metadata will be stored in **SQLite**.

Rationale:
- transactional
- local-first
- portable
- operationally light
- supports indexing and relational queries

### 4. Blob Store
Binary assets and large snapshots will be stored in a **content-addressed local blob store**.

Rationale:
- de-duplicates artifacts naturally
- avoids bloating SQLite
- simplifies artifact integrity verification

### 5. Retrieval Strategy
Contynu will implement retrieval in layers:
1. exact replay
2. structured memory lookup
3. semantic retrieval

Semantic retrieval is an enhancement layer, not the source of truth.

### 6. Rehydration Strategy
Rehydration will use a structured, deterministic packet generated from the canonical state and recent context.

The system will not depend on injecting the full raw transcript into every resumed model.

---

## Consequences

### Positive
- Strong performance and correctness foundation
- Clean separation between immutable truth and derived state
- Low operational complexity for local-first deployments
- Easier debugging and forensic traceability
- Better long-term extensibility across vendors and interfaces

### Negative
- More engineering complexity at the beginning than a pure scripting approach
- Rust raises the implementation bar relative to a Python-first prototype
- Multiple storage layers require disciplined schema and compatibility design

---

## Rejected Alternatives

### Python-only runtime
Rejected because it is faster to start but less ideal for a long-term runtime that must manage PTYs, filesystem activity, concurrency, and durable hot-path performance.

### SQLite-only canonical storage
Rejected because SQLite is useful for structured metadata but is less ideal as the sole canonical raw event log. A raw append-only journal remains simpler, more transparent, and easier to replay or export.

### Document database or cloud-first backend
Rejected because it adds operational weight and weakens local-first trust, which is central to the product.

### Embedding-first memory architecture
Rejected because semantic similarity alone is not sufficient for exact recovery, legal defensibility, or reproducible rehydration.

---

## Implementation Notes

The codebase should reflect the architectural boundary explicitly:
- `runtime` for capture and orchestration
- `journal` for immutable event writing and replay
- `store` for SQLite metadata and memory objects
- `blobs` for content-addressed artifact storage
- `rehydration` for packet generation
- `adapters` for CLI-specific normalization

Future language bindings and optional analysis utilities may be layered around the Rust core without changing the canonical storage model.
