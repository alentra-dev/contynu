# CLI Usage

## Commands

### `contynu <command...>`

Launches an ordinary terminal command directly inside Contynu’s runtime without requiring the `run` subcommand. This is the simplest generic wrapper path.

If the executable matches a configured LLM launcher in `.contynu/config.json`, Contynu will treat it as hydratable instead of a plain terminal command.

### `contynu codex [-- <args...>]`

Launches `codex` inside Contynu’s runtime using the primary project by default. When continuing an existing project, Contynu writes the rehydration packet to runtime files, exposes their paths via environment variables, and sends a startup prelude on stdin.

### `contynu claude [-- <args...>]`

Launches `claude` inside Contynu’s runtime using the primary project by default, with the same hydration delivery path as `codex`.

### `contynu gemini [-- <args...>]`

Launches `gemini` inside Contynu’s runtime using the primary project by default, with the same hydration delivery path as `codex`.

### `contynu init`

Creates the local state layout and initializes the SQLite metadata store.

### `contynu run -- <command...>`

Continues the primary project by default, wraps the external command, captures process lifecycle plus stdout/stderr, diffs workspace files before and after execution, records artifacts for large or binary outputs, and creates a checkpoint by default.

This remains available as the explicit form of the generic wrapper when users want a more self-documenting command.

Runtime behavior in this pass:

- stdout and stderr are captured incrementally while the wrapped process is still running
- each captured chunk is durably appended to the journal before the runtime continues
- accumulated stdout/stderr are registered as blob-backed artifacts after process exit
- workspace files are diffed before and after the run and recorded as canonical file events

### `contynu start-project`

Creates or returns the primary continuous project memory for the current state directory.

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

### `contynu repair [--project <id>]`

Repairs a truncated journal tail if needed, then reconciles journal state back into SQLite.

## Notes

- Contynu now models one continuous project memory per state directory by default.
- Raw project IDs remain available for exact scripting and advanced targeting.
- Known LLM launchers now have dedicated top-level commands so users do not need to remember `run -- <tool>`.
- Unknown future launchers can be configured in `.contynu/config.json`.
- Ordinary terminal commands can also be launched directly as `contynu <command...>`.
- Known LLM launchers receive continuity context via `CONTYNU_REHYDRATION_PACKET_FILE`, `CONTYNU_REHYDRATION_PROMPT_FILE`, and a startup stdin prelude when Contynu is continuing an existing project.
- `contynu run` uses a generic subprocess wrapper with real-time pipe capture rather than full PTY emulation.
- The adapter layer is model-agnostic and ready for native adapters, but only generic terminal wrapping is fully implemented in this pass.
