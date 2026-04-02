# Contynu Handoff Summary

## Architecture As Implemented

- Rust workspace with `contynu-core` and `contynu-cli`
- canonical append-only JSONL journal with checksum validation
- SQLite metadata store with migrations and journal reconciliation
- content-addressed blob store with deduplication and integrity verification
- checkpoint manager with manifest and rehydration packet generation
- config-authoritative launcher layer seeded with known LLM entries
- runtime wrapper with adapter detection, PTY-or-pipe transport selection, real-time stream capture, file classification, and post-turn memory derivation
- one primary continuous project memory per state directory by default

## Commands Available

- `contynu init`
- `contynu run -- <command...>`
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
- `contynu repair [--project <id>]`

## Known Limitations

- PTY transport currently uses the local `script` utility as the Unix implementation path
- interruption handling is best-effort rather than full signal choreography
- structured memory derivation is heuristic and intentionally lightweight rather than model-assisted
- checkpoint packets are deterministic but still heuristic in how mission and recent context are selected
- automatic rehydration injection exists through config-driven env/stdin/arg surfaces, but not yet through richer provider-native session APIs
- exact search is implemented; semantic retrieval remains intentionally deferred

## Next Best Steps

1. replace the `script`-based PTY path with a first-class PTY implementation and stronger stdin/signal choreography
2. add native adapter event mapping for Codex, Claude-style, and Gemini-style tools
3. deepen file tracking with better mime typing and artifact/source policy controls
4. evolve memory derivation from heuristics into richer deterministic extraction and supersession policies
5. add more integration coverage around resume/handoff workflows and repair semantics
