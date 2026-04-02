# Contynu Handoff Summary

## Architecture As Implemented

- Rust workspace with `contynu-core` and `contynu-cli`
- canonical append-only JSONL journal with checksum validation
- SQLite metadata store with migrations and journal reconciliation
- content-addressed blob store with deduplication and integrity verification
- checkpoint manager with manifest and rehydration packet generation
- config-authoritative launcher layer seeded with known LLM entries
- runtime wrapper with adapter detection, PTY-or-pipe transport selection, provider-native workspace context injection, real-time stream capture, file classification, and post-turn memory derivation
- one primary continuous project memory per state directory by default

## Commands Available

- `contynu init`
- `contynu codex [-- <args...>]`
- `contynu claude [-- <args...>]`
- `contynu gemini [-- <args...>]`
- `contynu run -- <command...>`
- `contynu status [--project <id>]`
- `contynu projects`
- `contynu recent [--limit <n>]`
- `contynu start-project`
- `contynu checkpoint [--project <id>]`
- `contynu resume [--project <id>]`
- `contynu handoff [--project <id>] --target-model <name>`
- `contynu replay [--project <id>]`
- `contynu inspect project [id]`
- `contynu inspect event <id>`
- `contynu search exact <query>`
- `contynu search memory <query>`
- `contynu artifacts list`
- `contynu doctor`
- `contynu config validate`
- `contynu config show`
- `contynu repair [--project <id>]`

## Known Limitations

- PTY transport is implemented in-process on Unix, but still needs deeper signal/process-group polish
- interruption handling is best-effort rather than full signal choreography
- structured memory derivation is heuristic and intentionally lightweight rather than model-assisted
- checkpoint packets are deterministic but still heuristic in how mission and recent context are selected
- provider-native rehydration currently targets workspace instruction files (`AGENTS.md`, `CLAUDE.md`, `GEMINI.md`) rather than deeper session-native APIs
- exact search is implemented; semantic retrieval remains intentionally deferred

## Next Best Steps

1. replace the `script`-based PTY path with a first-class PTY implementation and stronger stdin/signal choreography
2. add native adapter event mapping for Codex, Claude-style, and Gemini-style tools
3. deepen file tracking with better mime typing and artifact/source policy controls
4. evolve memory derivation from heuristics into richer deterministic extraction and supersession policies
5. add more integration coverage around resume/handoff workflows and repair semantics
