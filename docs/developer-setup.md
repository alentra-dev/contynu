# Developer Setup

## Prerequisites

- Rust toolchain with `cargo`, `rustfmt`, and standard test tooling
- a local filesystem where `.contynu/` state can be created
- Unix-like environment for the current in-process PTY implementation

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
contynu codex
contynu doctor
```

## Build and Release

```bash
cargo build --release -p contynu-cli
git tag v0.1.0
git push origin v0.1.0
```

Tagged releases publish prebuilt binaries and the user-facing installer scripts through the GitHub Actions release workflow.

## Development Notes

- the canonical journal is local JSONL and is safe to inspect manually
- SQLite is derived state and can be repaired from the journal for a session
- the hot path is intentionally explicit rather than hidden behind async infrastructure
- launcher behavior is primarily controlled through `.contynu/config.json`
- known launchers currently hydrate through workspace context files and runtime materialized packet files
- the PTY runtime is implemented in-process on Unix; pipe transport remains the fallback path
