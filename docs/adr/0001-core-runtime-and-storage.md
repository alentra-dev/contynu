# ADR 0001: Core Runtime and Storage Architecture

## Status
Accepted (with amendment — see note on journal decision)

## Context

Contynu is intended to be a high-trust, model-agnostic persistent memory layer for LLM workflows. The system must support persistent memory, deterministic recovery, and cross-model handoff.

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

### 2. ~~Canonical Event Store~~ (Superseded)

> **Amendment (April 2026):** The JSONL journal was removed in the v0.5.0 architecture rewrite. The original rationale for an append-only journal assumed that Contynu would derive memories by mining raw event streams. In practice, heuristic derivation from transcripts produced mostly noise (22/30 memories were process narration, not knowledge). The architecture now uses **model-driven memory** — AI models write memories directly via MCP tools. SQLite is the sole data store. The raw event stream was never consumed by any downstream system after derivation was removed, making the journal an artifact of a design assumption that no longer holds.

### 3. Metadata and Structured Memory
Structured metadata will be stored in **SQLite**.

Rationale:
- transactional
- local-first
- portable
- operationally light
- supports indexing and relational queries

In the current architecture, SQLite is the **primary and only data store** for memories, prompts, sessions, and checkpoints.

### 4. Blob Store
Binary assets and large snapshots will be stored in a **content-addressed local blob store**.

Rationale:
- de-duplicates artifacts naturally
- avoids bloating SQLite
- simplifies artifact integrity verification

### 5. Retrieval Strategy
Contynu will implement retrieval in layers:
1. structured memory lookup (model-written memories with importance scoring)
2. semantic retrieval (future enhancement)

### 6. Rehydration Strategy
Rehydration will use a structured, deterministic packet generated from model-written memories and recorded prompts.

The system will not depend on injecting the full raw transcript into every resumed model.

---

## Consequences

### Positive
- Strong performance and correctness foundation
- Model-driven memory eliminates heuristic noise
- Low operational complexity for local-first deployments
- Better long-term extensibility across vendors and interfaces
- Single data store (SQLite) simplifies operations

### Negative
- More engineering complexity at the beginning than a pure scripting approach
- Rust raises the implementation bar relative to a Python-first prototype
- Memory quality depends on model compliance with the MCP write contract

---

## Rejected Alternatives

### Python-only runtime
Rejected because it is faster to start but less ideal for a long-term runtime that must manage PTYs, filesystem activity, concurrency, and durable hot-path performance.

### JSONL journal as canonical storage (originally accepted, later reversed)
Originally accepted for simplicity and auditability. Reversed because the journal's primary consumer (the heuristic derivation engine) was removed when the architecture moved to model-driven memory. Without a consumer, the journal added I/O overhead and complexity with no value.

### Document database or cloud-first backend
Rejected because it adds operational weight and weakens local-first trust, which is central to the product.

### Embedding-first memory architecture
Rejected because semantic similarity alone is not sufficient for exact recovery or reproducible rehydration. May be added as an enhancement layer.

---

## Implementation Notes

The codebase should reflect the architectural boundary explicitly:
- `runtime` for process execution and hydration delivery
- `store` for SQLite metadata, memories, prompts, and sessions
- `mcp` for the MCP server with read and write tools
- `blobs` for content-addressed storage
- `checkpoint` for rehydration packet generation
- `rendering` for multi-format prompt rendering
- `adapters` for CLI-specific normalization
