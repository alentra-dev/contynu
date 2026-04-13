# CLI Usage

## Commands

### `contynu <command...>`

Launches an ordinary terminal command directly inside Contynu's runtime without requiring the `run` subcommand. This is the simplest generic wrapper path.

If the executable matches a configured LLM launcher in `.contynu/config.json`, Contynu will treat it as hydratable instead of a plain terminal command.

### `contynu codex [-- <args...>]`

Launches `codex` inside Contynu's runtime using the primary project by default. Continuity is injected primarily through a temporary `AGENTS.md` file with Codex-specific working continuation sections, plus environment variables that expose the sanitized packet and prompt files.

### `contynu claude [-- <args...>]`

Launches `claude` inside Contynu's runtime using the primary project by default. Continuity is injected via `--append-system-prompt` (XML format) and `--mcp-config .mcp.json` for MCP server registration.

### `contynu gemini [-- <args...>]`

Launches `gemini` inside Contynu's runtime using the primary project by default. Continuity is injected via a temporary `GEMINI.md` file (structured text format) and a `--prompt-interactive` startup nudge to read the memory file.

### Startup behavior

All interactive CLI entrypoints except `contynu mcp-server` perform a startup release check before normal dispatch. If a newer GitHub Release exists for the runtime OS/architecture of the currently running binary, Contynu offers:

- a manual update command that matches the current OS environment and install directory
- an auto-update flow that runs the matching release installer

`contynu mcp-server` skips the prompt because stdio output must remain clean for MCP transport.

### `contynu init`

Creates the local state layout and initializes the SQLite metadata store.

This also writes `.contynu/config.json` when it does not already exist, pre-populated with editable launcher entries for `codex`, `claude`, and `gemini`.

### `contynu run -- <command...>`

Continues the primary project by default, wraps the external command, and creates a checkpoint by default.

This remains available as the explicit form of the generic wrapper when users want a more self-documenting command.

Runtime behavior:
- configured LLM launchers can use PTY transport when `use_pty` is enabled
- configured launchers can install a temporary provider-native workspace context file when `context_file` is set
- rehydration packets include instructions for models to use MCP tools to write memories

### `contynu start-project`

Creates or returns the primary continuous project memory for the current state directory.

### `contynu status [--project <id>]`

Prints a compact structured view of the current project state, including memory counts and session metadata.

### `contynu projects`

Lists known projects in the current state directory and marks the primary project.

### `contynu recent [--limit <n>]`

Shows the most recently active projects with their latest session metadata.

### `contynu checkpoint [--project <id>]`

Builds a checkpoint manifest and deterministic rehydration packet for the primary project unless a project is explicitly selected.

### `contynu resume [--project <id>]`

Prints the rehydration packet JSON for resuming the same continuous project.

### `contynu handoff [--project <id>] --target-model <name>`

Prints a rehydration packet annotated for a target model switch.

### `contynu inspect project [id]`

Prints project metadata for the primary project, or an explicit project if provided.

### `contynu search memory <query>`

Runs text search against structured memory objects.

### `contynu ingest [--dry-run] [--tool <claude|codex|gemini>]`

Discovers unrecorded Claude Code, Codex, and Gemini session memory files and ingests them into the project archive. The startup MCP path also runs this automatically so the continuity layer can pick up external session state before the next model turn.

### `contynu distill [--project <id>]`

Runs Dream Phase candidate detection and prints related memory clusters that can be merged into Golden Facts with the `consolidate_memories` MCP tool.

### `contynu doctor`

Reports core storage paths and a minimal health summary.

### `contynu config validate`

Loads and validates `.contynu/config.json`, then prints the effective launcher configuration.

### `contynu config show`

Prints the raw config file contents.

### `contynu export-memory [--project <id>] [--max-chars <n>] [--with-markers]`

Outputs importance-ranked memories as Markdown. When `--with-markers` is set, wraps output in `<!-- contynu-memory-sync:start/end -->` HTML comment markers for OpenClaw MEMORY.md write-back.

### `contynu mcp-server [--state-dir <path>]`

Starts the Contynu MCP server using stdio JSON-RPC 2.0 transport. Exposes eight tools:

- `search_memory` — text search with kind, scope, time window, sort, and pagination
- `list_memories` — browse all memories with filtering and sorting
- `write_memory` — create a new structured memory
- `update_memory` — update an existing memory
- `delete_memory` — remove a memory
- `record_prompt` — record the user's prompt verbatim
- `suggest_consolidation` — find redundant clusters that are good consolidation candidates
- `consolidate_memories` — supersede multiple related memories with one Golden Fact

The MCP server auto-registers with Claude (`.mcp.json`), Codex (`config.toml`), and Gemini (`gemini mcp add`) on first LLM launch. On startup it also runs external session discovery so unrecorded Claude/Codex/Gemini session memories can be pulled into the archive before the next tool turn.

### `contynu openclaw setup [--openclaw-config <path>]`

One-time setup for OpenClaw integration. Registers the Contynu MCP server in OpenClaw's config, creates agent-to-project mapping, and prints plugin installation instructions.

### `contynu openclaw status`

Reports the health of the OpenClaw integration: state directory, primary project, mapped agents, active memories, and MCP server registration status.

## Notes

- Contynu models one continuous project memory per state directory by default.
- Raw project IDs remain available for exact scripting and advanced targeting.
- Known LLM launchers have dedicated top-level commands so users do not need to remember `run -- <tool>`.
- Unknown future launchers can be configured in `.contynu/config.json`.
- The generated config file is the preferred place to adjust known launcher behavior as upstream CLIs evolve.
- Configured launchers can request PTY transport with `use_pty`.
- Configured launchers can request provider-native workspace file injection with `context_file`.
- Configured launchers can choose `hydration_delivery` as `env_only`, `stdin_only`, or `env_and_stdin`.
- Configured launchers can also prepend `hydration_args` with placeholders such as `{prompt_file}`, `{packet_file}`, `{project_id}`, and `{schema_version}`.
- Ordinary terminal commands can also be launched directly as `contynu <command...>`.
- Known LLM launchers now primarily receive continuity through their configured provider-native workspace files plus runtime env/file materialization. Codex is `AGENTS.md`-first, Claude uses appended XML prompt text, and Gemini uses `GEMINI.md` plus a startup nudge. Generic stdin/env delivery remains available through config.
- `contynu run` uses Contynu's in-process PTY transport for launchers that request it and a pipe-based fallback otherwise.
- The adapter layer remains model-agnostic, but the launcher config is the authoritative place to tune startup surfaces for known and future tools.
