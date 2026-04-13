# Contynu Architecture Blueprint

## Purpose

Contynu is a model-agnostic persistent memory layer for LLM workflows. Its purpose is to persist, index, and rehydrate structured project memory so that sessions can survive crashes, resume seamlessly, and transfer across models without loss of meaningful context.

This document defines the production architecture for Contynu as of the v0.5+ model-driven memory rewrite and the subsequent invisible-continuity work.

---

## Product Principles

1. **Model-driven memory**
   The model decides what is worth remembering. Contynu stores what the model writes, not what a heuristic guesses.

2. **Model agnostic by default**
   Contynu must not depend on any one vendor's session semantics.

3. **Structured recall**
   The system supports structured memory retrieval. Memories are typed, scoped, and ranked for current relevance.

4. **Local-first trust model**
   The canonical state must be able to live entirely on the user's machine.

5. **Lean execution path**
   The hot path for memory persistence and retrieval must stay small, deterministic, and fast.

6. **Explicit rehydration**
   A resumed model should receive an authoritative state packet, not an ad hoc dump.

7. **Prompts always recorded**
   User prompts are recorded verbatim and never filtered or summarized away.

8. **Memory should feel invisible**
   Continuation should bias toward the smallest set of relevant durable context instead of broad document-shaped rehydration.

---

## System Overview

Contynu has four core subsystems:

1. **MCP Memory Interface**
   Exposes tools for models to write, update, delete, and query memories, and to record user prompts.

2. **Metadata Store**
   SQLite-backed storage for sessions, memory objects, prompts, checkpoints, working-set state, packet observations, ingestion tracking, and schema metadata.

3. **Rehydration Engine**
   Produces a deterministic state packet from model-written memories for resume, handoff, or model switch.

4. **Adapter Layer**
   Normalizes integration with different CLIs, agents, and tool ecosystems through launcher config and runtime materialization.

---

## High-Level Architecture

```text
User <-> Contynu Runtime Wrapper <-> Target LLM CLI / Agent
                |
                +-- MCP Server (write_memory, update_memory, delete_memory,
                |                record_prompt, search_memory, list_memories,
                |                suggest_consolidation, consolidate_memories)
                +-- Metadata Store (SQLite)
                +-- Blob Store (content-addressed)
                +-- Rehydration Packet Generator
```

The Contynu runtime sits between the user and the target LLM environment. Models write structured memories via MCP tools. User prompts are recorded verbatim. Rehydration packets are assembled from the accumulated memory store.

---

## Storage Design

### 1. Metadata Store

A local SQLite database is the primary structured store.

Core tables (schema v8):
- `sessions` — session metadata
- `memory_objects` — model-written memories with kind, scope, importance
- `prompts` — user prompts recorded verbatim with optional model interpretation
- `blobs` — blob metadata registry
- `checkpoints` — checkpoint manifests and rehydration references
- `working_set_entries` — carry-forward working set for next-packet relevance
- `packet_observations` — observability for why memories were included
- `ingested_sources` — external session ingestion tracking
- `schema_meta` — schema version tracking

SQLite is preferred for:
- low operational overhead
- transactional guarantees
- high enough performance for local-first single-user scenarios
- portability

### 2. Blob Store

Large or binary assets are stored separately in a content-addressed blob store.

Examples:
- rehydration packets
- uploaded files
- generated outputs

Blobs are keyed by SHA-256 digest.

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

### Memory Object
Represents a model-written reusable memory unit.

Fields:
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

### Prompt Record
Represents a user prompt recorded verbatim.

Fields:
- `prompt_id`
- `session_id`
- `verbatim` — the exact user input
- `interpretation` — model's interpretation if the prompt was ambiguous
- `interpretation_confidence`
- `source_model`
- `created_at`

---

## MCP Memory Interface

Models interact with Contynu's memory through eight MCP tools:

### Write Tools
- `write_memory` — create a new memory with kind, scope, importance, and reason
- `update_memory` — update an existing memory's text, importance, or reason
- `delete_memory` — remove a memory that is no longer relevant
- `record_prompt` — record the user's prompt verbatim with optional interpretation

### Read Tools
- `search_memory` — text search with kind, scope, time window, sort, and pagination
- `list_memories` — browse all memories with filtering and sorting

### Dream Phase Tools
- `suggest_consolidation` — find redundant memory clusters suitable for consolidation
- `consolidate_memories` — merge related memories into a Golden Fact while superseding originals

This replaces the previous heuristic-based memory derivation system. The model decides what matters; Contynu stores it faithfully and delivers it back on rehydration.

---

## Rehydration Design

Rehydration is one of Contynu's defining capabilities.

A fresh model or resumed process receives an authoritative rehydration packet assembled from model-written memories.

### Rehydration Packet Sections
1. Mission / project purpose
2. Current state summary
3. Recent changes since the last meaningful checkpoint
4. Constraints and preferences
5. Decision log
6. Open loops / pending tasks
7. Durable context selected by ranked relevance
8. Recent prompts
9. Retrieval instructions / memory query interface

### Rehydration Modes
- **Resume**: continue the same session after interruption
- **Handoff**: move to a different model or provider

### Model Instructions in Rehydration
Rehydration packets include explicit instructions telling the model how to use the MCP tools (including Dream Phase consolidation tools when relevant). These instructions are rendered in the model's preferred format: XML for Claude, AGENTS.md-first Markdown for Codex, and StructuredText for Gemini.

---

## Retrieval Design

Contynu supports structured retrieval through the MCP interface:

### Structured Retrieval
- by memory kind (fact, decision, constraint, todo, user_fact, project_knowledge)
- by scope (user, project, session)
- by time window
- by text search
- sorted by importance or recency

Packet selection also uses a persistent working set and prompt-aware ranking so recently useful memories can carry forward without repeated full-archive reconstruction.

Semantic retrieval via embeddings remains intentionally deferred.

---

## Adapter Architecture

Adapters translate tool-specific behavior into Contynu's normalized integration surface.

### Adapter Tiers
1. **Generic PTY Adapter**
   Works with most CLIs via terminal capture.

2. **Known LLM Adapter**
   Detects Claude, Codex, Gemini and delivers rehydration in each model's preferred format.

### Adapter Contract
Each adapter must define:
- detection
- launch behavior
- rehydration delivery format
- MCP server registration

---

## Checkpoint Strategy

Checkpoints are lightweight deterministic snapshots of project memory state.

Trigger points:
- before model handoff
- after substantial memory changes
- at periodic intervals during long-running sessions

Checkpoint contents:
- checkpoint ID
- memory snapshot
- rehydration packet

---

## Legacy Data Cleanup

On startup, Contynu detects and removes legacy storage artifacts from the v0.4.0 journal-based architecture:
- `journal/` directory
- `runtime/` directory
- Legacy DB tables (events, turns, files, artifacts)

This ensures a clean transition to the model-driven memory architecture.

---

## Security and Trust Model

### Baseline Requirements
- local-first by default
- no mandatory cloud dependency
- encryption at rest support
- configurable retention policies
- per-project isolation

---

## Implementation Stack

### Core Runtime Language
**Rust** for the capture runtime, storage, MCP server, and CLI.

### Storage
- SQLite for metadata, memories, prompts, and checkpoints
- Content-addressed local blob store for large content

---

## Build Order

### Foundation (completed)
1. SQLite metadata schema (v5)
2. Blob store
3. Checkpoint and rehydration packet generator
4. MCP server with memory write/read tools

### Runtime (completed)
5. Generic PTY runtime wrapper
6. Adapter detection and launcher integration
7. Model-aware rendering (XML/Markdown/StructuredText)

### Current Priorities
8. Strengthen rehydration packet quality from model-written memories
9. Expand MCP tool coverage
10. Deeper adapter integrations

---

## Summary

Contynu is built as a lean systems product:
- model-driven memory (the model decides what matters)
- deterministic recovery
- model-agnostic integration
- local-first trust
- structured retrieval
- rehydration as a first-class primitive
