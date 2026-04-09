# CLI Usage

## Commands

### `contynu <command...>`

Launches an ordinary terminal command directly inside Contynu’s runtime without requiring the `run` subcommand. This is the simplest generic wrapper path.

If the executable matches a configured LLM launcher in `.contynu/config.json`, Contynu will treat it as hydratable instead of a plain terminal command.

### `contynu codex [-- <args...>]`

Launches `codex` inside Contynu’s runtime using the primary project by default. Continuity is injected via a temporary `AGENTS.md` file (Markdown format) and stdin prelude, along with environment variables.

### `contynu claude [-- <args...>]`

Launches `claude` inside Contynu’s runtime using the primary project by default. Continuity is injected via `--append-system-prompt` (XML format) and `--mcp-config .mcp.json` for MCP server registration.

### `contynu gemini [-- <args...>]`

Launches `gemini` inside Contynu’s runtime using the primary project by default. Continuity is injected via a temporary `GEMINI.md` file (structured text format) and a `--prompt-interactive` startup nudge to read the memory file.

### `contynu init`

Creates the local state layout and initializes the SQLite metadata store.

This also writes `.contynu/config.json` when it does not already exist, pre-populated with editable launcher entries for `codex`, `claude`, and `gemini`.

### `contynu run -- <command...>`

Continues the primary project by default, wraps the external command, captures process lifecycle plus stdout/stderr, records stream artifacts, and creates a checkpoint by default.

This remains available as the explicit form of the generic wrapper when users want a more self-documenting command.

Runtime behavior in this pass:

- stdout and stderr are captured incrementally while the wrapped process is still running
- each captured chunk is durably appended to the journal before the runtime continues
- accumulated stdout/stderr are registered as blob-backed artifacts after process exit
- configured LLM launchers can use PTY transport when `use_pty` is enabled and `script` is available
- configured LLM launchers can use Contynu's built-in PTY transport when `use_pty` is enabled
- configured launchers can install a temporary provider-native workspace context file when `context_file` is set
- lightweight memory objects are derived after each turn for summary, facts, constraints, decisions, and todos

### `contynu start-project`

Creates or returns the primary continuous project memory for the current state directory.

### `contynu status [--project <id>]`

Prints a compact structured view of the current project state, including counts, latest turn metadata, and recent events.

### `contynu projects`

Lists known projects in the current state directory and marks the primary project.

### `contynu recent [--limit <n>]`

Shows the most recently active projects with their latest turn metadata.

### `contynu checkpoint [--project <id>]`

Builds a checkpoint manifest and deterministic rehydration packet for the primary project unless a project is explicitly selected.

### `contynu resume [--project <id>]`

Prints the rehydration packet JSON for resuming the same continuous project.

### `contynu handoff [--project <id>] --target-model <name>`

Prints a rehydration packet annotated for a target model switch.

### `contynu replay [--project <id>]`

Replays canonical journal events for a project with stored offsets and line numbers.

### `contynu inspect project [id]`

Prints the structured event index for the primary project, or an explicit project if provided.

### `contynu inspect event <id>`

Prints one indexed event.

### `contynu search exact <query>`

Runs exact text search against indexed event payload JSON and event types.

### `contynu search memory <query>`

Runs exact text search against structured memory objects.

### `contynu artifacts list`

Lists tracked artifacts, optionally scoped to one project.

### `contynu doctor`

Reports core storage paths and a minimal health summary.

### `contynu config validate`

Loads and validates `.contynu/config.json`, then prints the effective launcher configuration.

### `contynu config show`

Prints the raw config file contents.

### `contynu repair [--project <id>]`

Repairs a truncated journal tail if needed, then reconciles journal state back into SQLite.

### `contynu import <files...> [--format <auto|claude-jsonl|codex-jsonl|gemini|chatgpt|text>]`

Imports conversation history from external files into the project memory. Supports:

- **Claude JSONL** (`.jsonl` with role/content) — auto-detected
- **Codex rollout JSONL** (`.jsonl` with session_meta/response_item) — auto-detected
- **Gemini session JSON** (`.json` with sessionId/messages) — auto-detected
- **ChatGPT export JSON** (`.json` with mapping/message) — auto-detected
- **Plain text** — fallback for any other format

After import, memories are derived automatically and are immediately searchable.

### `contynu ingest [--project <id>] [--adapter <name>] [--model <name>] [--derive-memory]`

Accepts JSONL events from stdin and writes them to the project journal. Used by the OpenClaw plugin to feed conversation turns into Contynu. Each line is a JSON object with `event_type`, `actor`, and `payload` fields.

### `contynu export-memory [--project <id>] [--max-chars <n>] [--with-markers]`

Outputs importance-ranked memories as Markdown. When `--with-markers` is set, wraps output in `<!-- contynu-memory-sync:start/end -->` HTML comment markers for OpenClaw MEMORY.md write-back.

### `contynu mcp-server [--state-dir <path>]`

Starts the Contynu MCP server using stdio JSON-RPC 2.0 transport. Exposes three tools:

- `search_memory` — text search with kind, time window, sort, and pagination
- `list_memories` — browse all memories with filtering and sorting
- `search_events` — search raw event history with time window

The MCP server auto-registers with Claude (`.mcp.json`), Codex (`config.toml`), and Gemini (`gemini mcp add`) on first LLM launch.

### `contynu openclaw setup [--openclaw-config <path>]`

One-time setup for OpenClaw integration. Registers the Contynu MCP server in OpenClaw's config, creates agent-to-project mapping, and prints plugin installation instructions.

### `contynu openclaw status`

Reports the health of the OpenClaw integration: state directory, primary project, mapped agents, active memories, and MCP server registration status.

## Auto-Import

On every `contynu run`, `contynu claude`, `contynu codex`, or `contynu gemini` launch, Contynu automatically scans for existing session files:

- `~/.codex/sessions/**/rollout-*.jsonl` — Codex CLI rollout files
- `~/.gemini/tmp/*/chats/session-*.json` — Gemini CLI session files

New files are imported and tracked in `.contynu/imported-sessions.json` to avoid re-importing. This means installing Contynu and running it once makes all prior Codex and Gemini conversations immediately searchable.

## Notes

- Contynu now models one continuous project memory per state directory by default.
- Raw project IDs remain available for exact scripting and advanced targeting.
- Known LLM launchers now have dedicated top-level commands so users do not need to remember `run -- <tool>`.
- Unknown future launchers can be configured in `.contynu/config.json`.
- The generated config file is the preferred place to adjust known launcher behavior as upstream CLIs evolve.
- Configured launchers can request PTY transport with `use_pty`.
- Configured launchers can request provider-native workspace file injection with `context_file`.
- Configured launchers can choose `hydration_delivery` as `env_only`, `stdin_only`, or `env_and_stdin`.
- Configured launchers can also prepend `hydration_args` with placeholders such as `{prompt_file}`, `{packet_file}`, `{project_id}`, and `{schema_version}`.
- Ordinary terminal commands can also be launched directly as `contynu <command...>`.
- Known LLM launchers now primarily receive continuity through their configured provider-native workspace files plus runtime env/file materialization. Generic stdin/env delivery remains available through config.
- `contynu run` uses PTY transport for launchers that request it and a pipe-based fallback otherwise.
- `contynu run` uses Contynu's in-process PTY transport for launchers that request it and a pipe-based fallback otherwise.
- The adapter layer remains model-agnostic, but the launcher config is now the authoritative place to tune startup surfaces for known and future tools.
