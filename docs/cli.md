# CLI Usage

## Commands

### `contynu init`

Creates the local state layout and initializes the SQLite metadata store.

### `contynu run -- <command...>`

Starts a new session, wraps the external command, captures process lifecycle plus stdout/stderr, diffs workspace files before and after execution, records artifacts for large or binary outputs, and creates a checkpoint by default.

### `contynu start-session`

Allocates a session record without running a wrapped command.

### `contynu checkpoint --session <id>`

Builds a checkpoint manifest and deterministic rehydration packet for the session.

### `contynu resume --session <id>`

Prints the rehydration packet JSON for resuming the same session.

### `contynu handoff --session <id> --target-model <name>`

Prints a rehydration packet annotated for a target model switch.

### `contynu replay --session <id>`

Replays canonical journal events for a session with stored offsets and line numbers.

### `contynu inspect session <id>`

Prints the structured event index for a session from SQLite.

### `contynu inspect event <id>`

Prints one indexed event.

### `contynu search exact <query>`

Runs exact text search against indexed event payload JSON and event types.

### `contynu search memory <query>`

Runs exact text search against structured memory objects.

### `contynu artifacts list`

Lists tracked artifacts, optionally scoped to one session.

### `contynu doctor`

Reports core storage paths and a minimal health summary.

### `contynu repair --session <id>`

Repairs a truncated journal tail if needed, then reconciles journal state back into SQLite.

## Notes

- `contynu run` currently uses a generic subprocess wrapper with captured pipes rather than full PTY emulation.
- The adapter layer is model-agnostic and ready for native adapters, but only generic terminal wrapping is fully implemented in this pass.
