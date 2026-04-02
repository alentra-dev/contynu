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
    prj_<id>.jsonl
  sqlite/
    contynu.db
  blobs/
    sha256/ab/cd/<digest>
  checkpoints/
    prj_<id>/
      chk_<id>/
        manifest.json
        rehydration.json
```

## CLI

### Streamlined LLM launch

```bash
contynu codex
contynu claude
contynu gemini
```

Known LLM launcher commands automatically attach to the current project memory and use the same durable capture path as `run`.
When a known LLM launcher is continuing an existing project, Contynu now materializes a rehydration packet and injects continuity through the launcher’s configured startup surface. The seeded config uses:

- `AGENTS.md` for `codex`
- `CLAUDE.md` for `claude`
- `GEMINI.md` for `gemini`

Those launchers also request PTY transport by default so interactive sessions run against a real terminal.

Unknown future LLM CLIs can be taught to Contynu through `.contynu/config.json`. If a launcher is listed there, the normal direct path like `contynu myllm` will recognize it as hydratable.
`contynu init` now writes a starter `.contynu/config.json` that already includes `codex`, `claude`, and `gemini`, so those integrations can be adjusted locally as upstream CLIs change.

Example:

```json
{
  "llm_launchers": [
    {
      "command": "futurellm",
      "aliases": ["futurellm-cli"],
      "hydrate": true,
      "use_pty": true,
      "context_file": "FUTURELLM.md",
      "hydration_delivery": "env_only",
      "hydration_args": ["--context-file", "{prompt_file}", "--project", "{project_id}"],
      "extra_env": {
        "FUTURELLM_MODE": "enabled"
      }
    }
  ]
}
```

`hydration_delivery` supports `env_only`, `stdin_only`, or `env_and_stdin`.
`hydration_args` lets a configured launcher receive rehydration context through adapter-specific CLI flags using placeholders like `{prompt_file}`, `{packet_file}`, `{project_id}`, and `{schema_version}`.
`use_pty` lets a launcher request PTY transport when available.
`context_file` lets a launcher use a provider-native workspace instruction file that Contynu installs for the duration of the run and then restores.

### Streamlined Generic Launch

```bash
contynu cargo test
contynu bash -lc "make build"
```

Ordinary terminal commands can also be launched directly. Contynu treats them as generic wrapped commands inside the same project continuity stream.

### Initialize state

```bash
contynu init
```

### Wrap an external command

```bash
contynu run -- cargo test
```

`contynu run` is still available as the explicit generic wrapper form. It captures streams incrementally while the process is running, durably appends them to the journal in real time, classifies changed files as source/generated/artifact outputs, and derives lightweight structured memory after each turn.

### Create or inspect recovery state

```bash
contynu start-project
contynu checkpoint
contynu resume
contynu handoff --target-model gpt-5.4
contynu replay
```

### Inspect and repair

```bash
contynu inspect project
contynu inspect event evt_<id>
contynu search exact journal
contynu search memory decision
contynu artifacts list
contynu doctor
contynu repair
```

Contynu now defaults to a single continuous project memory per state directory. A raw `project_id` still exists for exact targeting and scripting, but normal commands resolve the primary project automatically.

## Developer Setup

```bash
cargo test
cargo fmt --check
```

More detailed docs:

- [`docs/implementation-plan.md`](docs/implementation-plan.md)
- [`docs/cli.md`](docs/cli.md)
- [`docs/adapter-architecture.md`](docs/adapter-architecture.md)
- [`docs/crash-recovery.md`](docs/crash-recovery.md)
- [`docs/rehydration.md`](docs/rehydration.md)
- [`docs/handoff-summary.md`](docs/handoff-summary.md)

## License

This repository is licensed under the Mozilla Public License 2.0. See [`LICENSE`](./LICENSE).
