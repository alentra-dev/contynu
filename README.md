# Contynu

**Memory that persists.** Model-agnostic persistent memory for AI coding assistants.

Contynu gives Claude, Codex, and Gemini persistent memory that transfers across sessions and models. AI models write their own memories via MCP tools — facts, decisions, constraints, and context — so the next model picks up exactly where the last one left off.

**Website:** [contynu.com](https://contynu.com)

## Install

### Linux / macOS

```bash
curl -fsSL https://github.com/alentra-dev/contynu/releases/latest/download/install.sh | sh
```

### Windows

```powershell
irm https://github.com/alentra-dev/contynu/releases/latest/download/install.ps1 | iex
```

### From source

```bash
cargo install --path crates/contynu-cli
```

## Quick Start

```bash
# Use with any AI coding tool — just prefix with contynu
contynu claude     # wraps Claude Code with persistent memory
contynu codex      # wraps Codex CLI — picks up where Claude left off
contynu gemini     # wraps Gemini CLI — has full context from both
```

That's it. No configuration needed. Contynu auto-detects the AI tool, registers an MCP server, and delivers memory on the next session.

## How It Works

1. **Launch** — Contynu wraps your AI tool and registers an MCP server for memory access
2. **Write** — The AI model writes memories via MCP tools (`write_memory`, `record_prompt`) at each generation stop. The model decides what's worth remembering — no heuristics, no noise
3. **Transfer** — When you switch models, Contynu delivers accumulated memory in each model's optimal format (XML for Claude, Markdown for Codex, structured text for Gemini)
4. **Recall** — Models search and browse the full memory archive via `search_memory` and `list_memories` MCP tools

## Key Features

- **Model-driven memory** — AI models write their own memories via MCP tools. The model decides what's worth remembering
- **Cross-model memory transfer** — Facts from Claude are available in Codex and Gemini
- **Scoped memory system** — User scope (follows you everywhere), project scope (this codebase only), session scope (ephemeral)
- **Six memory kinds** — fact, constraint, decision, todo, user_fact, project_knowledge
- **Model-aware rendering** — Each model receives context in its optimal format
- **MCP server** — 6 tools: `write_memory`, `update_memory`, `delete_memory`, `record_prompt`, `search_memory`, `list_memories`
- **Auto-registration** — MCP server registers itself with each CLI automatically
- **Prompt recording** — Every user prompt recorded verbatim with optional model interpretation
- **Local-first** — All data stays on your machine. SQLite + content-addressed blobs
- **Zero config** — Replace `claude` with `contynu claude`. Works immediately

## OpenClaw Integration

Contynu provides permanent memory for [OpenClaw](https://github.com/openclaw/openclaw) agents via the `contynu-openclaw` plugin.

### Setup

```bash
# One-time setup
contynu openclaw setup

# Install the plugin
npm install -g contynu-openclaw
```

### What It Does

- **Per-agent memory isolation** — each agent gets its own Contynu project
- **Protects against compaction loss** — checkpoints before compaction fires
- **Writes back to MEMORY.md** — importance-ranked facts in OpenClaw's native format
- **MCP tools for deep recall** — agents can write, search, and update the full memory archive
- **Model-agnostic** — works with any model OpenClaw supports

### OpenClaw Issues Addressed

| Issue | Problem | With Contynu |
|-------|---------|-------------|
| #5429 | 45 hours lost to silent compaction | Pre-compaction checkpoint — nothing lost |
| #7477 | Compaction fails silently | MEMORY.md + MCP ensure context always available |
| #25947 | Safety constraints deleted | Constraints ranked highest, persist in MEMORY.md |
| #31781 | No importance-based memory | Model-assigned importance scores |
| #39885 | No session memory | Persistent forever in SQLite |

See [`packages/contynu-openclaw/`](packages/contynu-openclaw/) for the plugin source.

## Architecture

- **Memory store:** SQLite with WAL mode (schema v5) — sessions, memory_objects, prompts, blobs, checkpoints
- **Large content:** Content-addressed local blob store (SHA-256)
- **Recovery:** Deterministic rehydration packets from model-written memories
- **Memory:** Model-driven with scoped kinds, importance ratings, and provenance
- **MCP:** Stdio JSON-RPC server with read and write tools
- **Runtime:** Local CLI wrapper with PTY/pipe transport

### Storage Layout

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

## CLI Reference

### LLM Launch

```bash
contynu claude                    # Launch Claude with persistent memory
contynu codex                     # Launch Codex with persistent memory
contynu gemini                    # Launch Gemini with persistent memory
contynu run -- cargo test         # Wrap any command
contynu cargo test                # Direct passthrough
```

### Memory & Checkpoints

```bash
contynu status                    # Project state snapshot
contynu checkpoint                # Create manual checkpoint
contynu resume                    # Build rehydration packet
contynu handoff --target-model gpt-5.4  # Prepare for model switch
contynu search memory "auth"      # Search memory objects
contynu export-memory             # Export as Markdown
```

### MCP Server

```bash
contynu mcp-server                # Start stdio MCP server (used by LLM CLIs)
```

The MCP server auto-registers with Claude (`.mcp.json`), Codex (`config.toml`), and Gemini (`gemini mcp add`) on first launch. Models can then call `write_memory`, `update_memory`, `delete_memory`, `record_prompt`, `search_memory`, and `list_memories` tools directly.

### OpenClaw Integration

```bash
contynu openclaw setup            # Configure Contynu for OpenClaw
contynu openclaw status           # Check integration health
```

### Other Commands

```bash
contynu init                      # Initialize state directory
contynu projects                  # List all projects
contynu inspect project           # Inspect project details
contynu doctor                    # Diagnostic info
contynu config validate           # Validate launcher config
```

### Custom Launchers

Unknown LLM CLIs can be taught to Contynu via `.contynu/config.json`:

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
      "extra_env": { "FUTURELLM_MODE": "enabled" },
      "prompt_format": "markdown"
    }
  ]
}
```

## Documentation

- [`docs/contynu-technical-overview.md`](docs/contynu-technical-overview.md) — Complete technical reference
- [`docs/architecture.md`](docs/architecture.md) — System design blueprint
- [`docs/cli.md`](docs/cli.md) — CLI command reference
- [`docs/adapter-architecture.md`](docs/adapter-architecture.md) — Adapter system design
- [`docs/rehydration.md`](docs/rehydration.md) — Rehydration packet structure
- [`docs/crash-recovery.md`](docs/crash-recovery.md) — Durability and recovery
- [`docs/handoff-summary.md`](docs/handoff-summary.md) — Model handoff capabilities
- [`packages/contynu-openclaw/`](packages/contynu-openclaw/) — OpenClaw plugin

## Developer Setup

```bash
cargo test
cargo fmt --check
cd packages/contynu-openclaw && npm install && npm test
```

## Creators

- **Udonna Eke-Okoro** — Creator
- **Kelenna Eke-Okoro** — Co-Creator

## License

This repository is licensed under the Mozilla Public License 2.0. See [`LICENSE`](./LICENSE).

## Disclaimer

This software is provided "as is", without warranty of any kind, express or implied, including but not limited to the warranties of merchantability, fitness for a particular purpose, and noninfringement. In no event shall the authors or copyright holders be liable for any claim, damages, or other liability, whether in an action of contract, tort, or otherwise, arising from, out of, or in connection with the software or the use or other dealings in the software. Use at your own risk. Contynu stores data locally on your machine; you are solely responsible for the security and backup of your data.
