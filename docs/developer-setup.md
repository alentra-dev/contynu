# Developer Setup

## Prerequisites

- Rust toolchain with `cargo`, `rustfmt`, and standard test tooling
- a local filesystem where `.contynu/` state can be created
- Unix-like environment for the in-process PTY implementation

## Core Commands

```bash
cargo fmt
cargo test
```

## Useful Local Flows

```bash
contynu init
contynu config validate
contynu status
contynu claude
contynu doctor
```

## Build and Release

```bash
cargo build --release -p contynu-cli
git tag v0.5.1
git push origin v0.5.1
```

Tagged releases publish prebuilt binaries and the user-facing installer scripts through the GitHub Actions release workflow.

## Development Notes

- SQLite is the primary data store — memories, prompts, sessions, checkpoints
- the hot path is intentionally explicit rather than hidden behind async infrastructure
- launcher behavior is primarily controlled through `.contynu/config.json`
- known launchers currently hydrate through workspace context files and runtime materialized packet files
- the PTY runtime is implemented in-process on Unix; pipe transport remains the fallback path
- the MCP server opens the database in read-write mode (required for write_memory, record_prompt tools)
- old architecture data (journals, events, turns) is automatically cleaned up on first access after upgrade
