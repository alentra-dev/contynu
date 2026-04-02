# Crash Recovery Semantics

## Canonical Recovery Rule

The append-only JSONL journal is the source of truth. Recovery always begins there.

## Tail Repair

When a journal is opened:

1. Contynu scans the file from the beginning.
2. Each complete newline-terminated record is parsed and checksum-validated.
3. If the final record is truncated or invalid, the journal is truncated back to the last valid byte offset.
4. Valid prefix entries remain authoritative.

This makes truncated-tail crashes recoverable without rewriting valid history.

## SQLite Reconciliation

SQLite is treated as derived structured state. If SQLite is incomplete or stale relative to the project journal:

1. repair or verify the journal
2. replay canonical events
3. upsert indexed rows back into SQLite

The `contynu repair --project <id>` command performs this recovery path. If no project is given, Contynu repairs the primary project.

## Streaming Runtime Durability

For wrapped command execution, Contynu persists captured stream output as the process runs rather than only after exit. This reduces the amount of volatile work that can be lost if the runtime itself crashes mid-command.

For PTY-backed launchers, the merged PTY stream is captured incrementally through the same canonical event path.

## Current Limitations

- Mid-file corruption is surfaced as an error rather than silently repaired.
- Recovery events are not yet emitted automatically as separate canonical records.
- PTY interruption handling is improved but still not a complete process-group choreography layer for every edge case.
