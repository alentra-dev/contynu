# Contynu

Contynu is a model-agnostic persistent memory layer for LLM workflows.

It captures prompts, responses, tool activity, artifacts, file changes, and execution metadata into a durable local continuity layer so work can resume cleanly across crashes, restarts, and model handoffs.

## Current Architecture

- Canonical truth: append-only JSONL journal
- Structured metadata: SQLite
- Large and binary content: content-addressed local blob store
- Recovery primitive: deterministic rehydration packet
- Runtime shape: local-first CLI wrapper around external tools

The implementation in this repository is intentionally explicit and inspectable. The journal is authoritative, SQLite is derived structured state, and checkpoint/rehydration artifacts are materialized locally.

## Workspace Layout

```text
.contynu/
  journal/
    ses_<id>.jsonl
  sqlite/
    contynu.db
  blobs/
    sha256/ab/cd/<digest>
  checkpoints/
    ses_<id>/
      chk_<id>/
        manifest.json
        rehydration.json
```

## CLI

### Initialize state

```bash
contynu init
```

### Wrap an external command

```bash
contynu run -- cargo test
```

### Create or inspect recovery state

```bash
contynu checkpoint --session ses_<id>
contynu resume --session ses_<id>
contynu handoff --session ses_<id> --target-model gpt-5.4
contynu replay --session ses_<id>
```

### Inspect and repair

```bash
contynu inspect session ses_<id>
contynu inspect event evt_<id>
contynu search exact journal
contynu search memory decision
contynu artifacts list
contynu doctor
contynu repair --session ses_<id>
```

## Developer Setup

```bash
cargo test
cargo fmt --check
```

More detailed docs:

- [`docs/implementation-plan.md`](docs/implementation-plan.md)
- [`docs/cli.md`](docs/cli.md)
- [`docs/crash-recovery.md`](docs/crash-recovery.md)
- [`docs/rehydration.md`](docs/rehydration.md)
- [`docs/adapter-architecture.md`](docs/adapter-architecture.md)
- [`docs/handoff-summary.md`](docs/handoff-summary.md)

## License

This repository is licensed under the Mozilla Public License 2.0. See [`LICENSE`](./LICENSE).
