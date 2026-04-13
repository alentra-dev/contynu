# Contynu — Complete Technical Overview

**Version:** 0.5.1+ | **Created:** April 2026 | **Language:** Rust + TypeScript (OpenClaw plugin)
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
11. [Codebase Structure](#11-codebase-structure)
12. [CLI Commands](#12-cli-commands)
13. [Installation & Deployment](#13-installation--deployment)
14. [Technical Specifications](#14-technical-specifications)

---

## 1. What Is Contynu

Contynu is a **model-agnostic persistent memory layer** for AI coding assistants.

When you work with AI tools like Claude, Codex, or Gemini, each tool forgets everything the moment you close it. If you switch between tools, they have no idea what the other one said.

Contynu provides MCP tools that let AI models **write their own memories** — facts, decisions, constraints, and context — and **delivers those memories** to whichever model you use next. All your AI tools share one continuous memory.

---

## 2. The Problem It Solves

### The Reality of Multi-Model AI Workflows

Most serious AI users work with 2-3 different models:

- **Claude** for deep reasoning and complex architecture
- **Codex (GPT)** for fast code generation and execution
- **Gemini** for web-connected research and broad analysis

Each model has strengths. But every time you switch, you hit a **cold start** — the new model has zero context about your project.

### What Contynu Does

| Without Contynu | With Contynu |
|----------------|-------------|
| Claude decides on JWT auth | Claude decides on JWT auth and writes it to memory |
| You switch to Codex | You switch to Codex |
| Codex: "What auth system?" | Codex: "Continuing with JWT auth using HMAC-SHA256 as decided" |

---

## 3. How It Works — The Big Picture

```
┌─────────┐     ┌──────────┐     ┌─────────┐
│  Claude  │────▶│          │────▶│  Codex  │
│ Session 1│     │ CONTYNU  │     │Session 2│
└─────────┘     │          │     └─────────┘
                │  stores   │
                │ memories  │     ┌─────────┐
                │ delivers  │────▶│ Gemini  │
                │ context   │     │Session 3│
                └──────────┘     └─────────┘
```

**Step by step:**

1. **You launch:** `contynu claude` (instead of just `claude`)
2. **You work:** Have a normal conversation. The model writes important memories via MCP tools at each stop point.
3. **You switch:** `contynu codex` — Contynu delivers Claude's memories to Codex in Markdown format.
4. **Codex knows:** Every decision Claude made is available. Zero re-explaining.
5. **Repeat:** Switch to Gemini, back to Claude — memory accumulates and transfers every time.

---

## 4. Architecture

### Design Principles

1. **Model-Driven Memory** — AI models write their own memories. No heuristic extraction, no transcript mining.
2. **Model Agnostic** — No dependency on any single LLM vendor. Works with any CLI tool.
3. **Local-First** — All data stays on the user's machine. No cloud, no API keys, no data leakage.
4. **Explicit Rehydration** — Models receive structured context packets, not random data dumps.

### System Overview

```
┌─────────────────────────────────────────────────────┐
│                   contynu CLI                        │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │ Adapter   │  │ Runtime  │  │ MCP Server       │  │
│  │ Detection │  │ Engine   │  │ (read + write)   │  │
│  └──────────┘  └──────────┘  └──────────────────┘  │
├─────────────────────────────────────────────────────┤
│                  contynu-core                        │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │ Memory   │  │Checkpoint│  │ Rendering        │  │
│  │ Store    │  │ Manager  │  │ (XML/MD/Text)    │  │
│  └──────────┘  └──────────┘  └──────────────────┘  │
├─────────────────────────────────────────────────────┤
│                  Storage Layer                       │
│  ┌──────────────────────┐  ┌──────────────────┐    │
│  │ SQLite (memories,    │  │ Blob Store       │    │
│  │  prompts, sessions)  │  │ (SHA-256)        │    │
│  └──────────────────────┘  └──────────────────┘    │
└─────────────────────────────────────────────────────┘
```

---

## 5. Core Concepts

### Project / Session
A **project** is a continuous timeline of work identified by a unique ID (e.g., `prj_019d5528...`). All sessions with different models share the same project, so memory accumulates across model switches.

### Memory Object
A structured piece of knowledge written by the AI model:

| Kind | Scope | Description | Example |
|------|-------|-------------|---------|
| **fact** | project | Something true about the project | "The API uses JWT authentication" |
| **decision** | project | A choice that was made | "Use HMAC-SHA256 for token signing" |
| **constraint** | project | A rule or limitation | "Must support backward compatibility" |
| **todo** | project | An unfinished task | "Implement token refresh endpoint" |
| **user_fact** | user | Something about the user | "Senior engineer, prefers terse output" |
| **project_knowledge** | project | Technical discovery | "V3 API returns 403; use V4 for search" |

### Memory Scope
- **user** — follows the user across all projects
- **project** — scoped to the current codebase
- **session** — ephemeral, auto-expires

### Prompt Record
Every user prompt is recorded verbatim. If the prompt is ambiguous, the model writes its interpretation alongside.

### Checkpoint
A snapshot of the project's memory state. Contains a **rehydration packet** — the structured context delivered to the next model.

---

## 6. Memory System

### How Memories Are Created

AI models write memories directly via MCP tools at each generation stop point. There are no heuristics, no transcript mining, and no automated extraction.

**The model decides:**
- Whether anything from its output is worth remembering
- What kind of memory it is (fact, decision, constraint, todo, user_fact, project_knowledge)
- What scope it has (user, project, session)
- How important it is (0.0 to 1.0)
- Why it's worth remembering (free-text reason)

**The user's prompt is always recorded** — the model never decides to skip it. If the prompt is ambiguous, the model writes its interpretation.

### MCP Tools

| Tool | Purpose |
|------|---------|
| `write_memory` | Write a new memory with kind, scope, importance, and reason |
| `update_memory` | Correct or refine an existing memory by ID |
| `delete_memory` | Remove a memory that is no longer relevant |
| `record_prompt` | Record the user's verbatim prompt with optional interpretation |
| `search_memory` | Search memories by text, kind, scope, time window |
| `list_memories` | Browse all active memories with filtering and pagination |
| `suggest_consolidation` | Find redundant memory clusters suitable for Golden Fact consolidation |
| `consolidate_memories` | Merge related memories into a single Golden Fact while superseding originals |

### Importance

The model assigns importance directly (0.0 to 1.0). Contynu does not re-score or override this value. When building rehydration packets, memories are selected within a configurable packet budget using a blended relevance score that combines importance, lexical relevance to the latest prompt, recency, access count, scope, and working-set carry-forward state.

---

## 7. Cross-Model Transfer

### Rendering Formats

Each model receives context in its optimal format:

| Model | Format | Delivered As |
|-------|--------|-------------|
| Claude | XML | `<contynu_memory>` with nested sections |
| Codex/GPT | Markdown | AGENTS.md-first working continuation block plus env/file artifacts |
| Gemini | StructuredText | Labeled sections with bullet points |

### Model Instructions

Every rehydration prompt includes explicit instructions telling the model how to use the Contynu MCP tools. The model knows it should:
- Call `record_prompt` with the user's input at each stop point
- Call `write_memory` for facts, decisions, and constraints worth recalling
- Call `search_memory` before writing to avoid duplicates
- Call `update_memory` to correct existing memories
- Call Dream Phase consolidation tools when redundant memory clusters should be merged

---

## 8. MCP Server

The MCP server runs as a stdio JSON-RPC 2.0 transport, typically launched by the AI tool's MCP integration.

### Auto-Registration

On first launch, Contynu registers itself as an MCP server with:
- Claude Code → `.mcp.json`
- Codex CLI → `~/.codex/config.toml`
- Gemini CLI → `gemini mcp add`
- OpenClaw → `openclaw.json`

### Panic Safety

Each request is wrapped in `catch_unwind` so a single bad request never crashes the server. The stdio transport stays open across all calls.

On MCP startup, Contynu also discovers unrecorded Claude Code, Codex, and Gemini session memory files and ingests missing session state into the archive before the next model turn.

### Resources

- `contynu://project/brief` — Sanitized AI-facing rehydration packet
- `contynu://project/recent` — Recent prompts

---

## 9. Adapter System

Contynu auto-detects which AI tool is being launched and configures hydration delivery accordingly.

### Built-in Adapters

| Adapter | Hydration | Transport |
|---------|-----------|-----------|
| Claude | env_only + `--append-system-prompt` | PTY |
| Codex | env_only with AGENTS.md-first continuation | PTY |
| Gemini | env_and_stdin + `--prompt-interactive` | PTY |

### Custom Adapters

Configure in `.contynu/config.json`:

```json
{
  "llm_launchers": [{
    "command": "futurellm",
    "hydrate": true,
    "hydration_delivery": "env_and_stdin",
    "prompt_format": "markdown"
  }]
}
```

---

## 10. Storage Layer

### SQLite (Schema v8)

The sole data store. Core tables include:

| Table | Purpose |
|-------|---------|
| `sessions` | Project metadata (ID, status, CLI, model, timestamps) |
| `memory_objects` | Model-written memories with kind, scope, importance, reason |
| `prompts` | User prompts with verbatim text and model interpretation |
| `blobs` | Content-addressed large content (SHA-256) |
| `checkpoints` | Recovery bundles with manifest and rehydration packet |
| `working_set_entries` | Carry-forward working set for the next packet |
| `packet_observations` | Packet selection observability and hygiene signals |
| `ingested_sources` | External session ingestion tracking |

### Blob Store

Content-addressed by SHA-256 hash. File layout: `blobs/sha256/{a}/{b}/{digest}`. Atomic writes via temp file + rename.

### Migration

On first access after upgrade, Contynu automatically:
1. Drops legacy tables (events, turns, files, artifacts) if present
2. Removes old journal/ and runtime/ directories
3. Applies the latest schema and migration additions, including working-set and packet-observation tables

---

## 11. Codebase Structure

```text
crates/
  contynu-core/          # Core library
    src/
      mcp.rs             # MCP server with 8 tools (read + write + Dream Phase)
      store.rs           # SQLite operations (memories, prompts, sessions)
      checkpoint.rs      # Rehydration packet builder
      rendering.rs       # Multi-format prompt rendering (XML/MD/Text)
      runtime.rs         # Process execution, hydration delivery
      adapters.rs        # Launcher detection and configuration
      config.rs          # Config loading and validation
      blobs.rs           # Content-addressed blob store
      state.rs           # Directory layout, old data cleanup
      ids.rs             # Typed ID generation (prj_, mem_, chk_, etc.)
      text.rs            # UTF-8 safety utilities
      pty.rs             # Unix PTY spawning
      error.rs           # Error types
  contynu-cli/           # CLI binary
    src/
      main.rs            # Command dispatcher
      mcp_server.rs      # Stdio MCP transport
      mcp_registration.rs # Auto-register with Claude/Codex/Gemini/OpenClaw
    tests/
      smoke.rs           # Integration tests
packages/
  contynu-openclaw/      # OpenClaw plugin (TypeScript)
```

---

## 12. CLI Commands

### LLM Launch
- `contynu claude [-- <args>]` — Launch Claude with persistent memory
- `contynu codex [-- <args>]` — Launch Codex with persistent memory
- `contynu gemini [-- <args>]` — Launch Gemini with persistent memory
- `contynu run -- <command>` — Wrap any command
- `contynu <command>` — Direct passthrough

### Memory & Checkpoints
- `contynu status` — Project state snapshot
- `contynu checkpoint` — Create manual checkpoint
- `contynu resume` — Build rehydration packet
- `contynu handoff --target-model <name>` — Prepare for model switch
- `contynu search memory <query>` — Search memory objects
- `contynu export-memory` — Export as Markdown

### Infrastructure
- `contynu init` — Initialize state directory
- `contynu start-project` — Create new primary project
- `contynu projects` — List all projects
- `contynu inspect project` — Inspect project details
- `contynu doctor` — Diagnostic info
- `contynu config validate` / `config show` — Configuration
- `contynu mcp-server` — Start MCP server
- `contynu openclaw setup` / `openclaw status` — OpenClaw integration

---

## 13. Installation & Deployment

### Install

```bash
# Linux / macOS
curl -fsSL https://github.com/alentra-dev/contynu/releases/latest/download/install.sh | sh

# Windows
irm https://github.com/alentra-dev/contynu/releases/latest/download/install.ps1 | iex

# From source
cargo install --path crates/contynu-cli
```

### Distribution

Prebuilt binaries via GitHub Releases for Linux, macOS, and Windows (x86_64 + ARM).

---

## 14. Technical Specifications

### ID System

| Prefix | Type | Format |
|--------|------|--------|
| `prj_` | Project | UUIDv7 (32 hex chars) |
| `mem_` | Memory | UUIDv7 |
| `chk_` | Checkpoint | UUIDv7 |

### Schema Version
Current: **v5** (model-driven memory architecture)

### Test Coverage
- 36 unit tests (contynu-core)
- 8 integration tests (contynu-cli smoke tests)
- Coverage: store CRUD, MCP tools, checkpoint generation, rendering, runtime execution

### Dependencies
- **Rust** core: chrono, rusqlite, serde, serde_json, sha2, uuid, clap, ctrlc, libc, thiserror
- **TypeScript** plugin: Node.js >= 22.0.0

### Performance
- SQLite WAL mode for concurrent reads
- Content-addressed blob dedup
- Configurable token budgets for rehydration packets (default: 4000 tokens, max 20 per category)
