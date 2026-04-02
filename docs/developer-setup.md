# Developer Setup

## Prerequisites

- Rust toolchain with `cargo`, `rustfmt`, and standard test tooling
- a local filesystem where `.contynu/` state can be created

## Core Commands

```bash
cargo fmt
cargo test
```

## Development Notes

- the canonical journal is local JSONL and is safe to inspect manually
- SQLite is derived state and can be repaired from the journal for a session
- the hot path is intentionally explicit rather than hidden behind async infrastructure
- native model-specific adapters are not the current implementation focus; preserve normalized canonical events first
