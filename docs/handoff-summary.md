# Contynu Handoff Summary

## Architecture As Implemented

- Rust workspace with `contynu-core` and `contynu-cli`
- canonical append-only JSONL journal with checksum validation
- SQLite metadata store with migrations and journal reconciliation
- content-addressed blob store with deduplication and integrity verification
- checkpoint manager with manifest and rehydration packet generation
- generic subprocess runtime wrapper with adapter detection and file diff capture

## Commands Available

- `contynu init`
- `contynu run -- <command...>`
- `contynu start-session`
- `contynu checkpoint --session <id>`
- `contynu resume --session <id>`
- `contynu handoff --session <id> --target-model <name>`
- `contynu replay --session <id>`
- `contynu inspect session <id>`
- `contynu inspect event <id>`
- `contynu search exact <query>`
- `contynu search memory <query>`
- `contynu artifacts list`
- `contynu doctor`
- `contynu repair --session <id>`

## Known Limitations

- runtime wrapping currently uses captured pipes rather than a full PTY session
- interruption handling is best-effort rather than full signal choreography
- structured memory derivation is manual/API-driven; automatic memory extraction is not yet implemented
- checkpoint packets are deterministic but still heuristic in how mission and recent context are selected
- exact search is implemented; semantic retrieval remains intentionally deferred

## Next Best Steps

1. implement a full PTY-backed runtime path for interactive CLIs
2. add native adapter event mapping for Codex, Claude-style, and Gemini-style tools
3. enrich file tracking with stronger generated-artifact classification and diff storage policy
4. add automatic memory extraction and supersession logic
5. add more integration coverage around resume/handoff workflows and repair semantics
