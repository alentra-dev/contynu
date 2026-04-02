# CLI Usage

## Commands

### `contynu <command...>`

Launches an ordinary terminal command directly inside Contynu’s runtime without requiring the `run` subcommand. This is the simplest generic wrapper path.

If the executable matches a configured LLM launcher in `.contynu/config.json`, Contynu will treat it as hydratable instead of a plain terminal command.

### `contynu codex [-- <args...>]`

Launches `codex` inside Contynu’s runtime using the primary project by default. The seeded launcher config injects continuity through a temporary `AGENTS.md` file in the workspace, along with runtime files and environment variables.

### `contynu claude [-- <args...>]`

Launches `claude` inside Contynu’s runtime using the primary project by default. The seeded launcher config injects continuity through a temporary `CLAUDE.md` file in the workspace.

### `contynu gemini [-- <args...>]`

Launches `gemini` inside Contynu’s runtime using the primary project by default. The seeded launcher config injects continuity through a temporary `GEMINI.md` file in the workspace.

### `contynu init`

Creates the local state layout and initializes the SQLite metadata store.

This also writes `.contynu/config.json` when it does not already exist, pre-populated with editable launcher entries for `codex`, `claude`, and `gemini`.

### `contynu run -- <command...>`

Continues the primary project by default, wraps the external command, captures process lifecycle plus stdout/stderr, diffs workspace files before and after execution, records artifacts for large or binary outputs, and creates a checkpoint by default.

This remains available as the explicit form of the generic wrapper when users want a more self-documenting command.

Runtime behavior in this pass:

- stdout and stderr are captured incrementally while the wrapped process is still running
- each captured chunk is durably appended to the journal before the runtime continues
- accumulated stdout/stderr are registered as blob-backed artifacts after process exit
- configured LLM launchers can use PTY transport when `use_pty` is enabled and `script` is available
- configured LLM launchers can use Contynu's built-in PTY transport when `use_pty` is enabled
- configured launchers can install a temporary provider-native workspace context file when `context_file` is set
- workspace files are diffed before and after the run, classified as source/generated/artifact outputs, and recorded as canonical file events
- lightweight memory objects are derived after each turn for summary, facts, todos, and file notes

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
