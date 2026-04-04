# Contynu

**Memory that persists.** Model-agnostic persistent memory for LLM workflows.

Contynu captures prompts, responses, tool activity, command output, artifacts, and execution metadata into a durable local continuity layer so work can resume cleanly across crashes, restarts, and model handoffs between Claude, Codex, Gemini, and any future LLM CLI.

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

Prebuilt binaries are available for:
- Linux (x86_64, aarch64)
- macOS (x86_64, Apple Silicon)
- Windows (x86_64, aarch64)

You can override the install directory or version:

```bash
CONTYNU_INSTALL_DIR="$HOME/bin" CONTYNU_VERSION="v0.2.1" sh ./scripts/install.sh
```

## Quick Start

```bash
# Use with any LLM CLI — just prefix with contynu
contynu claude     # wraps Claude Code with persistent memory
contynu codex      # wraps Codex CLI — picks up where Claude left off
contynu gemini     # wraps Gemini CLI — has full context from both
```

That's it. No configuration needed. Contynu auto-detects the LLM, captures the session, and transfers memory on the next handoff.

## How It Works

1. **Capture** — Contynu wraps your LLM CLI and records every interaction to an append-only journal
2. **Extract** — Facts, decisions, constraints, and context are extracted as structured memory objects
3. **Transfer** — When you switch models, Contynu delivers the accumulated memory in the format each model understands best (XML for Claude, Markdown for Codex, structured text for Gemini)
4. **Recall** — An MCP server lets any model search the full project history on demand

## Key Features

- **Cross-model memory transfer** — Facts from Claude are available in Codex and Gemini
- **Importance-ranked memories** — Scored by importance, recency, and confidence so critical decisions always transfer
- **Model-aware rendering** — Each model receives context in its optimal format
- **MCP server** — LLMs query the full memory archive via `search_memory`, `list_memories`, and `search_events`
- **Auto-registration** — MCP server registers itself with each CLI automatically
- **Indefinite memory** — Append-only JSONL journal with SHA-256 checksums; nothing is ever lost
- **Local-first** — All data stays on your machine. SQLite + JSONL + content-addressed blobs
- **Zero config** — Replace `claude` with `contynu claude`. Works immediately

## Architecture

- **Canonical truth:** Append-only JSONL journal (one per project)
- **Structured metadata:** SQLite with WAL mode
- **Large content:** Content-addressed local blob store (SHA-256)
- **Recovery:** Deterministic rehydration packets with budget-aware assembly
- **Memory:** Typed objects (Fact, Constraint, Decision, Todo, Summary) with importance scoring and provenance
- **MCP:** Stdio JSON-RPC server with search, list, and event query tools
- **Runtime:** Local CLI wrapper with PTY/pipe/script-based capture

### Storage Layout

```text
.contynu/
  journal/
    prj_<id>.jsonl
  sqlite/
    contynu.db
  blobs/
    sha256/ab/cd/<digest>
  checkpoints/
    prj_<id>/
      chk_<id>/
        manifest.json
        rehydration.json
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
contynu search exact "JWT"        # Search event payloads
```

### MCP Server

```bash
contynu mcp-server                # Start stdio MCP server (used by LLM CLIs)
```

The MCP server auto-registers with Claude (`.mcp.json`), Codex (`config.toml`), and Gemini (`gemini mcp add`) on first launch. LLMs can then call `search_memory`, `list_memories`, and `search_events` tools directly.

### Other Commands

```bash
contynu init                      # Initialize state directory
contynu projects                  # List all projects
contynu recent                    # Recent activity
contynu replay                    # Canonical event sequence
contynu inspect project           # Inspect project details
contynu inspect event evt_<id>    # Inspect specific event
contynu artifacts list            # List tracked artifacts
contynu doctor                    # Diagnostic info
contynu repair                    # Fix corrupted journals
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

## Developer Setup

```bash
cargo test
cargo fmt --check
```

## Documentation

- [`docs/architecture.md`](docs/architecture.md) — System design blueprint
- [`docs/cli.md`](docs/cli.md) — CLI command reference
- [`docs/adapter-architecture.md`](docs/adapter-architecture.md) — Adapter system design
- [`docs/rehydration.md`](docs/rehydration.md) — Rehydration packet structure
- [`docs/crash-recovery.md`](docs/crash-recovery.md) — Durability and recovery
- [`docs/handoff-summary.md`](docs/handoff-summary.md) — Model handoff capabilities

## Creators

- **Udonna Eke-Okoro** — Creator
- **Kelenna Eke-Okoro** — Co-Creator

## License

This repository is licensed under the Mozilla Public License 2.0. See [`LICENSE`](./LICENSE).

## Disclaimer

This software is provided "as is", without warranty of any kind, express or implied, including but not limited to the warranties of merchantability, fitness for a particular purpose, and noninfringement. In no event shall the authors or copyright holders be liable for any claim, damages, or other liability, whether in an action of contract, tort, or otherwise, arising from, out of, or in connection with the software or the use or other dealings in the software. Use at your own risk. Contynu stores data locally on your machine; you are solely responsible for the security and backup of your data.
