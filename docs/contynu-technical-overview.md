# Contynu — Complete Technical Overview

**Version:** 0.2.1 | **Created:** April 2026 | **Language:** Rust
**Creators:** Udonna Eke-Okoro & Kelenna Eke-Okoro
**License:** Mozilla Public License 2.0
**Website:** [contynu.com](https://contynu.com) | **Source:** [github.com/alentra-dev/contynu](https://github.com/alentra-dev/contynu)

---

## Table of Contents

1. [What Is Contynu](#1-what-is-contynu)
2. [The Problem It Solves](#2-the-problem-it-solves)
3. [How It Works — The Big Picture](#3-how-it-works--the-big-picture)
4. [Architecture](#4-architecture)
5. [Core Concepts](#5-core-concepts)
6. [Memory System](#6-memory-system)
7. [Cross-Model Transfer](#7-cross-model-transfer)
8. [MCP Server](#8-mcp-server)
9. [Adapter System](#9-adapter-system)
10. [Storage Layer](#10-storage-layer)
11. [Event Model](#11-event-model)
12. [Codebase Structure](#12-codebase-structure)
13. [CLI Commands](#13-cli-commands)
14. [Installation & Deployment](#14-installation--deployment)
15. [Brand & Website](#15-brand--website)
16. [Technical Specifications](#16-technical-specifications)

---

## 1. What Is Contynu

Contynu is a **model-agnostic persistent memory layer** for LLM (Large Language Model) workflows.

In plain terms: when you work with AI coding tools like Claude, Codex, or Gemini, each tool forgets everything the moment you close it. And if you switch between tools, they have no idea what the other one said.

Contynu sits between you and your AI tools. It **captures everything**, **remembers it forever**, and **delivers the right context** to whichever model you use next. The result is that all your AI tools share one continuous memory — like they're the same assistant with a perfect memory.

---

## 2. The Problem It Solves

### The Reality of Multi-Model AI Workflows

Most serious AI users today work with 2-3 different models:

- **Claude** for deep reasoning and complex architecture
- **Codex (GPT)** for fast code generation and execution
- **Gemini** for web-connected research and broad analysis

Each model has strengths. But every time you switch, you hit a **cold start** — the new model has zero context about your project, your decisions, or what the previous model did. You end up re-explaining everything from scratch.

### What Contynu Does

| Without Contynu | With Contynu |
|----------------|-------------|
| Claude decides on JWT auth | Claude decides on JWT auth |
| You switch to Codex | You switch to Codex |
| Codex: "What auth system?" | Codex: "Continuing with JWT auth using HMAC-SHA256 as decided" |
| You re-explain everything | Codex already knows everything |

---

## 3. How It Works — The Big Picture

```
┌─────────┐     ┌──────────┐     ┌─────────┐
│  Claude  │────▶│          │────▶│  Codex  │
│ Session 1│     │ CONTYNU  │     │Session 2│
└─────────┘     │          │     └─────────┘
                │ captures  │
                │ extracts  │     ┌─────────┐
                │ transfers │────▶│ Gemini  │
                │ recalls   │     │Session 3│
                └──────────┘     └─────────┘
```

**Step by step:**

1. **You launch:** `contynu claude` (instead of just `claude`)
2. **You work:** Have a normal conversation with Claude. Contynu captures everything in the background.
3. **You exit:** Contynu extracts structured memories — facts, decisions, constraints — and stores them.
4. **You switch:** `contynu codex` — Contynu delivers Claude's memories to Codex in Markdown format.
5. **Codex knows:** Every decision Claude made is available to Codex. Zero re-explaining.
6. **Repeat:** Switch to Gemini, back to Claude — memory accumulates and transfers every time.

---

## 4. Architecture

### Design Principles

1. **Durability First** — Every event is persisted before acknowledgment. Nothing is lost.
2. **Model Agnostic** — No dependency on any single LLM vendor. Works with any CLI tool.
3. **Local-First** — All data stays on the user's machine. No cloud, no API keys, no data leakage.
4. **Append-Only Truth** — The event journal is immutable. What happened, happened.
5. **Explicit Rehydration** — Models receive structured context packets, not random data dumps.

### System Overview

```
┌─────────────────────────────────────────────────────┐
│                   contynu CLI                        │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │ Adapter   │  │ Runtime  │  │ MCP Server       │  │
│  │ Detection │  │ Engine   │  │ (search/recall)  │  │
│  └──────────┘  └──────────┘  └──────────────────┘  │
├─────────────────────────────────────────────────────┤
│                  contynu-core                        │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │ Memory   │  │Checkpoint│  │ Rendering        │  │
│  │ Scoring  │  │ Manager  │  │ (XML/MD/Text)    │  │
│  └──────────┘  └──────────┘  └──────────────────┘  │
├─────────────────────────────────────────────────────┤
│                  Storage Layer                       │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │ JSONL    │  │ SQLite   │  │ Blob Store       │  │
│  │ Journal  │  │ Metadata │  │ (SHA-256)        │  │
│  └──────────┘  └──────────┘  └──────────────────┘  │
└─────────────────────────────────────────────────────┘
```

---

## 5. Core Concepts

### Project / Session
A **project** is a continuous timeline of work. It's identified by a unique ID (e.g., `prj_019d5528...`). All sessions with different models share the same project, so memory accumulates.

### Turn
A **turn** is one invocation of an LLM — from launch to exit. A project has many turns.

### Event
An **event** is an atomic captured action — a user message, an assistant response, a file change, a tool call. Events are immutable and checksummed.

### Memory Object
A structured piece of knowledge extracted from events:

| Kind | Description | Example |
|------|-------------|---------|
| **Fact** | Something true about the project | "The API uses JWT authentication" |
| **Decision** | A choice that was made | "Use HMAC-SHA256 for token signing" |
| **Constraint** | A rule or limitation | "Must support backward compatibility" |
| **Todo** | An unfinished task | "Implement token refresh endpoint" |
| **Summary** | A session overview | "Last turn built the auth middleware" |

### Checkpoint
A snapshot of the project's semantic state at a point in time. Contains a **rehydration packet** — the structured context that gets delivered to the next model.

---

## 6. Memory System

### How Memories Are Created

Contynu extracts memories from LLM output using three strategies:

1. **Prefix Matching** (highest confidence: 0.95)
   - Lines starting with `Fact:`, `Decision:`, `Constraint:`, `Todo:`
   - These are explicit markers that LLMs sometimes produce naturally

2. **Markdown Header Extraction** (confidence: 0.90)
   - Content under `### Facts`, `### Decisions`, etc. headers
   - Bullet items are extracted as individual memories

3. **Key Phrase Detection** (confidence: 0.75)
   - "We decided to..." → Decision
   - "Must always..." / "Never..." → Constraint
   - "Note:" / "Important:" → Fact
   - "Still need to..." / "Next step..." → Todo

### Importance Scoring

Every memory has an importance score (0.0 to 1.0):

| Kind | Default Importance | Rationale |
|------|-------------------|-----------|
| Constraint | 0.9 | Almost always critical |
| Decision | 0.85 | High-value, shapes project direction |
| Todo | 0.75 | Actionable items |
| Fact | 0.7 | Useful context |
| Summary | 0.4 | Replaceable with each new turn |

### Budget-Aware Selection

When building a rehydration packet, memories are ranked by:

```
relevance = importance × 0.5 + recency × 0.3 + confidence × 0.2
```

The top memories are selected within a configurable token budget (default: 4000 tokens, max 20 per category). This means the most important facts always make it into the handoff, regardless of project size.

### Memory Consolidation

When active memory count exceeds 50, Contynu automatically consolidates:
1. Groups related memories by keyword overlap
2. Merges low-importance groups into single consolidated memories
3. Preserves the highest importance from each group
4. Tracks which memories were consolidated (provenance chain)

### Provenance

Every memory records:
- **source_adapter** — which CLI produced it (claude_cli, codex_cli, gemini_cli)
- **source_model** — the specific model name (if available)
- **text_hash** — SHA-256 of the text for deduplication
- **access_count** — how many times it's been included in a packet

---

## 7. Cross-Model Transfer

### The Transfer Pipeline

```
Session N ends → Memory extraction → Checkpoint → Rehydration packet built
                                                         ↓
Session N+1 starts ← Context delivered ← Model-aware rendering
```

### Model-Aware Rendering

The same packet is rendered differently for each model:

**Claude receives XML:**
```xml
<contynu_memory project="prj_..." schema="2">
  <mission>Fix the authentication bug</mission>
  <facts>
    <fact source="codex_cli">The API uses JWT authentication</fact>
  </facts>
  <decisions>
    <decision source="claude_cli">Use HMAC-SHA256 for signing</decision>
  </decisions>
</contynu_memory>
```

**Codex receives Markdown:**
```markdown
# Contynu Memory Context
**Project:** prj_... | **Schema:** 2

## Stable Facts
- The API uses JWT authentication

## Decisions
- Use HMAC-SHA256 for signing
```

**Gemini receives Structured Text:**
```
IMPORTANT: This file contains project memory from prior sessions.

KEY FACTS FROM PRIOR SESSIONS:
  * The API uses JWT authentication

Contynu continuity context for gemini_cli.
Stable Facts
- The API uses JWT authentication
```

### Delivery Mechanisms

| Model | Primary Delivery | Secondary | Context File |
|-------|-----------------|-----------|-------------|
| Claude | `--append-system-prompt` + `--mcp-config` | Env vars | `.mcp.json` |
| Codex | `AGENTS.md` file in CWD | Stdin prelude | `AGENTS.md` |
| Gemini | `GEMINI.md` file + `--prompt-interactive` nudge | Stdin prelude | `GEMINI.md` |

Context files are written before the LLM launches and cleaned up (restored to original) after the session ends.

---

## 8. MCP Server

### What Is MCP?

The **Model Context Protocol** (MCP) is an open standard created by Anthropic that lets LLMs call external tools. Contynu implements an MCP server so LLMs can query the full memory archive on demand — not just the subset that fits in the rehydration packet.

### The Server

`contynu mcp-server` runs as a stdio JSON-RPC 2.0 server. It's synchronous (no async/tokio), opens the SQLite database read-only to avoid contention, and is started automatically by the LLM CLI when configured.

### Available Tools

| Tool | Parameters | What It Does |
|------|-----------|-------------|
| `search_memory` | query, kind, after, before, sort_by, limit, offset | Search memories by text, kind, time window. Paginated. |
| `list_memories` | kind, sort_by, limit, offset | Browse all active memories with filtering. |
| `search_events` | query, after, before, limit, offset | Search raw event history. |

### Available Resources

| URI | What It Returns |
|-----|----------------|
| `contynu://project/brief` | Full rehydration packet as JSON |
| `contynu://project/recent` | Last 5 turns with summaries |

### Auto-Registration

When `contynu claude/codex/gemini` launches, it automatically registers the MCP server with that CLI's configuration:

- **Claude:** Writes `.mcp.json` in the working directory
- **Codex:** Appends to `~/.codex/config.toml`
- **Gemini:** Runs `gemini mcp add contynu ...`

The registration includes the active project ID as an environment variable, so the MCP server always queries the right project.

---

## 9. Adapter System

### What Adapters Do

An adapter is the bridge between Contynu and a specific LLM CLI. It handles:
- Detecting which CLI is being launched
- Choosing the right transport (PTY, pipes, or script logging)
- Building the command with hydration arguments
- Delivering the rehydration context in the right format

### Built-in Adapters

| Adapter | CLI Commands | Transport | Prompt Format | Hydration |
|---------|-------------|-----------|---------------|-----------|
| ClaudeCli | `claude`, `claude-code` | PTY/Script | XML | `--append-system-prompt` + `--mcp-config` |
| CodexCli | `codex`, `codex-cli` | PTY/Script | Markdown | AGENTS.md + stdin |
| GeminiCli | `gemini`, `gemini-cli` | PTY/Script | StructuredText | GEMINI.md + `--prompt-interactive` |
| Terminal | (any other command) | Pipes | StructuredText | None |

### Custom Adapters

Any LLM CLI can be added via `.contynu/config.json`:

```json
{
  "llm_launchers": [
    {
      "command": "futurellm",
      "aliases": ["futurellm-cli"],
      "hydrate": true,
      "use_pty": true,
      "hydration_delivery": "env_and_stdin",
      "hydration_args": ["--context-file", "{prompt_file}"],
      "prompt_format": "markdown"
    }
  ]
}
```

### Transport Selection

| Condition | Transport | How It Works |
|-----------|-----------|-------------|
| Real terminal + LLM adapter | Script logging | Uses `script` command to capture while inheriting the terminal |
| Unix, no terminal | PTY | Pseudo-terminal with real-time mirroring |
| Non-Unix or no PTY | Pipes | Standard stdin/stdout/stderr pipes |

---

## 10. Storage Layer

### Three-Tier Storage

```
.contynu/
├── journal/                    ← Tier 1: Canonical truth
│   └── prj_<id>.jsonl         One file per project, append-only
├── sqlite/                     ← Tier 2: Queryable index
│   └── contynu.db             WAL mode, derived from journal
├── blobs/                      ← Tier 3: Large content
│   └── sha256/ab/cd/<digest>  Content-addressed, SHA-256 keyed
├── checkpoints/
│   └── prj_<id>/chk_<id>/
│       ├── manifest.json
│       └── rehydration.json
├── runtime/                    ← Temporary per-session files
│   └── prj_<id>/
└── config.json                 ← Launcher configuration
```

### Journal (JSONL)
- One file per project
- Each line is a complete JSON event with a checksum
- Monotonically increasing sequence numbers
- Auto-repairs truncated tails on open
- The journal is **the source of truth** — SQLite is derived

### SQLite Database
- Tables: sessions, turns, events, artifacts, files, checkpoints, memory_objects, blobs, schema_migrations, schema_meta
- WAL (Write-Ahead Logging) for concurrent access
- Migration system (currently at v2)
- Read-only mode available for the MCP server

### Blob Store
- Content-addressed by SHA-256
- Two-level directory sharding: `sha256/AB/CD/ABCDEF...`
- Atomic writes (temp file + rename)
- Stores rehydration packets, file snapshots, binary artifacts

---

## 11. Event Model

### Event Envelope

Every event has:

| Field | Description |
|-------|-------------|
| `event_id` | Unique ID (`evt_` prefix + UUID v7) |
| `session_id` | Which project this belongs to |
| `turn_id` | Which turn (optional) |
| `seq` | Monotonic sequence number |
| `ts` | ISO 8601 timestamp |
| `actor` | Who generated it (see below) |
| `event_type` | What happened (see below) |
| `payload` | JSON data specific to the event type |
| `checksum` | SHA-256 of the canonicalized event |

### Actors (7)

System, User, Assistant, Tool, Runtime, Filesystem, Adapter

### Event Types (37)

**Session lifecycle:** SessionStarted, SessionInterrupted, SessionResumed, SessionEnded
**Adapters:** AdapterAttached, AdapterDetached
**Turns:** TurnStarted, TurnCompleted, TurnFailed, TurnCancelled
**Messages:** MessageInput, MessageOutput, MessageChunk, MessageRedaction
**Tools:** ToolCall, ToolResult, ToolStream, ToolError
**I/O Capture:** StdinCaptured, StdoutCaptured, StderrCaptured
**Process:** ProcessStarted, ProcessExited
**Files:** FileObserved, FileSnapshot, FileDiff, FileDeleted, WorkspaceScanCompleted
**Artifacts:** ArtifactRegistered, ArtifactMaterialized, ArtifactRead, ArtifactDeleted
**Checkpoints:** CheckpointCreated, RehydrationPacketCreated
**Memory:** MemoryObjectDerived, MemoryObjectSuperseded, MemoryConsolidated, HandoffAssessed

---

## 12. Codebase Structure

### Workspace

```
contynu/
├── crates/
│   ├── contynu-core/          ← Library: all business logic
│   │   └── src/
│   │       ├── lib.rs         Public API (36 lines)
│   │       ├── runtime.rs     Execution engine (2,637 lines)
│   │       ├── store.rs       SQLite metadata store (1,525 lines)
│   │       ├── checkpoint.rs  Checkpoint & rehydration (811 lines)
│   │       ├── mcp.rs         MCP server dispatcher (628 lines)
│   │       ├── rendering.rs   Model-aware prompt rendering (506 lines)
│   │       ├── adapters.rs    LLM CLI adapter system (490 lines)
│   │       ├── event.rs       Event model & checksums (336 lines)
│   │       ├── config.rs      Launcher configuration (316 lines)
│   │       ├── journal.rs     Append-only JSONL journal (253 lines)
│   │       ├── files.rs       File tracking & diffing (230 lines)
│   │       ├── pty.rs         Unix pseudo-terminal (205 lines)
│   │       ├── blobs.rs       Content-addressed blob store (106 lines)
│   │       ├── ids.rs         Typed ID system (101 lines)
│   │       ├── state.rs       Filesystem path management (81 lines)
│   │       └── error.rs       Error types (41 lines)
│   │
│   └── contynu-cli/           ← Binary: CLI entry point
│       └── src/
│           ├── main.rs        CLI commands & dispatch (951 lines)
│           ├── mcp_server.rs  Stdio JSON-RPC loop (69 lines)
│           └── mcp_registration.rs  Auto-registration (171 lines)
│
├── brand-assets/              ← Logo, favicon, brand guide (SVG)
├── docs/                      ← Architecture & design docs
├── scripts/                   ← Install scripts (sh + ps1)
├── site/                      ← contynu.com landing page
├── sql/                       ← SQLite schema definitions
└── .github/workflows/         ← Release automation
```

**Total: ~10,000 lines of Rust** across 19 source files.

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `rusqlite` (bundled) | SQLite with compiled-in engine |
| `serde` + `serde_json` | JSON serialization |
| `clap` | CLI argument parsing |
| `chrono` | Timestamps |
| `sha2` | SHA-256 checksums |
| `uuid` | UUID v7 ID generation |
| `libc` | Unix PTY support |

### Test Suite

- **32 unit tests** in contynu-core (memory, checkpoints, rendering, MCP, etc.)
- **8 integration tests** in contynu-cli (smoke tests for all commands)
- **Total: 40 tests**, all passing

---

## 13. CLI Commands

### Primary Usage

```bash
contynu claude          # Work with Claude (persistent memory)
contynu codex           # Work with Codex (inherits Claude's memory)
contynu gemini          # Work with Gemini (inherits all memory)
contynu <any command>   # Wrap any command with capture
```

### Full Command Reference

| Command | Description |
|---------|-------------|
| `contynu init` | Initialize state directory |
| `contynu claude/codex/gemini` | Launch LLM with persistent memory |
| `contynu run -- <cmd>` | Wrap any command |
| `contynu status` | Project state snapshot |
| `contynu projects` | List all projects |
| `contynu recent` | Recent activity |
| `contynu checkpoint` | Create manual checkpoint |
| `contynu resume` | Build rehydration packet |
| `contynu handoff --target-model <model>` | Prepare model switch |
| `contynu replay` | Show canonical event sequence |
| `contynu search memory <query>` | Search memory objects |
| `contynu search exact <query>` | Search event payloads |
| `contynu inspect project` | Inspect project details |
| `contynu inspect event <id>` | Inspect specific event |
| `contynu artifacts list` | List tracked artifacts |
| `contynu doctor` | Diagnostic info |
| `contynu repair` | Fix corrupted journals |
| `contynu config validate` | Validate launcher config |
| `contynu mcp-server` | Start MCP server (stdio) |

---

## 14. Installation & Deployment

### Install Methods

**Linux / macOS:**
```bash
curl -fsSL https://github.com/alentra-dev/contynu/releases/latest/download/install.sh | sh
```

**Windows (PowerShell):**
```powershell
irm https://github.com/alentra-dev/contynu/releases/latest/download/install.ps1 | iex
```

**From source:**
```bash
cargo install --path crates/contynu-cli
```

### Release Targets

| Platform | Architecture | Archive |
|----------|-------------|---------|
| Linux | x86_64 | `contynu-linux-x86_64.tar.gz` |
| Linux | aarch64 (ARM) | `contynu-linux-aarch64.tar.gz` |
| macOS | x86_64 (Intel) | `contynu-macos-x86_64.tar.gz` |
| macOS | aarch64 (Apple Silicon) | `contynu-macos-aarch64.tar.gz` |
| Windows | x86_64 | `contynu-windows-x86_64.zip` |
| Windows | aarch64 (ARM) | `contynu-windows-aarch64.zip` |

### Release Process

1. Tag: `git tag v0.x.x && git push origin v0.x.x`
2. GitHub Actions builds all 6 targets
3. Creates GitHub Release with binaries, install scripts, and checksums
4. Users install via the curl/irm one-liners

### Website

- **Domain:** contynu.com (Hetzner cloud, nginx, Let's Encrypt SSL)
- **Stack:** Static HTML + CSS, no JavaScript framework
- **Deployment:** `scp` to `/var/www/contynu/`

---

## 15. Brand & Website

### Brand Identity

- **Name:** contynu (always lowercase)
- **Tagline:** "Memory That Persists"
- **Logo concept:** A continuous unbroken path forming an abstract "C" with an internal loop. Three nodes represent model handoff points. The central dot is the persistent memory core.

### Color Palette

| Name | Hex | Usage |
|------|-----|-------|
| Indigo | #6366F1 | Primary brand color |
| Sky | #0EA5E9 | Secondary / gradient end |
| Violet | #818CF8 | Accent |
| Slate 900 | #0F172A | Dark backgrounds |
| Slate 50 | #F8FAFC | Light backgrounds |

### Typography

- **Primary:** Inter (semibold 600)
- **Monospace:** SF Mono / Fira Code / JetBrains Mono

### Assets (all SVG)

| File | Description |
|------|-------------|
| `logo-mark.svg` | Primary mark with gradient |
| `logo-mark-mono.svg` | Monochrome (uses currentColor) |
| `favicon.svg` | Simplified for 16x16/32x32 |
| `wordmark.svg` | "contynu" with gradient |
| `wordmark-dark.svg` | White wordmark for dark backgrounds |
| `logo-full.svg` | Mark + wordmark + tagline |
| `brand-guide.svg` | Visual reference sheet |

---

## 16. Technical Specifications

### By the Numbers

| Metric | Value |
|--------|-------|
| Total lines of code | ~10,000 (Rust) |
| Source files | 19 |
| Dependencies | 15 crates |
| Event types | 37 |
| Memory object kinds | 7 |
| MCP tools | 3 |
| Rendering formats | 3 (XML, Markdown, StructuredText) |
| Built-in adapters | 3 (Claude, Codex, Gemini) |
| Release targets | 6 (3 OS × 2 arch) |
| Test count | 40 |
| Git commits | 76+ |

### Schema Versions

| Version | What It Added |
|---------|--------------|
| v1 | Core tables: sessions, turns, events, artifacts, files, checkpoints, memory_objects, blobs |
| v2 | Memory enhancement: source_adapter, source_model, importance, access_count, last_accessed_at, consolidated_from_json, text_hash + indexes |

### Rehydration Packet (Schema v2)

```json
{
  "schema_version": 2,
  "project_id": "prj_...",
  "target_model": null,
  "mission": "Current user goal",
  "stable_facts": ["..."],
  "constraints": ["..."],
  "decisions": ["..."],
  "current_state": "Where things stand",
  "open_loops": ["..."],
  "relevant_artifacts": [{"path": "...", "kind": "...", "sha256": "..."}],
  "relevant_files": ["..."],
  "recent_verbatim_context": ["User: ...", "Assistant: ..."],
  "retrieval_guidance": ["..."],
  "memory_provenance": [{"memory_id": "...", "kind": "...", "importance": 0.85}]
}
```

### ID System

All IDs use UUID v7 (time-ordered) with typed prefixes:

| Prefix | Entity |
|--------|--------|
| `prj_` | Project/Session |
| `trn_` | Turn |
| `evt_` | Event |
| `art_` | Artifact |
| `chk_` | Checkpoint |
| `fil_` | File |
| `mem_` | Memory Object |

---

*This document reflects Contynu v0.2.1 as of April 2026.*
