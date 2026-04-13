# Crash Recovery Semantics

## Canonical Recovery Rule

SQLite with WAL (Write-Ahead Logging) mode is the primary durability mechanism. Recovery relies on SQLite's built-in crash safety guarantees.

## SQLite WAL Recovery

SQLite's WAL mode provides automatic crash recovery:

1. Uncommitted transactions are rolled back automatically on next open.
2. Committed data is always preserved.
3. No manual repair is needed for standard crash scenarios.

## Legacy Data Cleanup

On startup, Contynu detects and removes legacy v0.4.0 storage artifacts:
- `journal/` directory and JSONL files
- `runtime/` directory
- Legacy DB tables (events, turns, files, artifacts)

This is a one-time cleanup that happens automatically.

## Current Limitations

- PTY interruption handling is improved but still not a complete process-group choreography layer for every edge case.
- If the process crashes during an MCP tool call, the in-flight memory write may be lost, but all previously committed memories are safe.
