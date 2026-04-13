# Contynu Handoff Summary

## Architecture As Implemented

- Rust workspace with `contynu-core` and `contynu-cli`
- Model-driven memory via MCP tools (write_memory, update_memory, delete_memory, record_prompt)
- SQLite metadata store (schema v5) with sessions, memory_objects, prompts, checkpoints, blobs
- Content-addressed blob store with deduplication and integrity verification
- Checkpoint manager with manifest and rehydration packet generation
- Config-authoritative launcher layer seeded with known LLM entries
- Runtime wrapper with adapter detection, PTY-or-pipe transport selection, provider-native workspace context injection, and MCP server registration
- One primary continuous project memory per state directory by default
- Memory scopes (user, project, session) and 6 memory kinds
- User prompts always recorded verbatim via record_prompt
- Legacy data cleanup on startup (journal/, runtime/, old DB tables)
- Multi-format rendering with model instructions (XML/Markdown/StructuredText)

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
- `contynu inspect project [id]`
- `contynu search memory <query>`
- `contynu doctor`
- `contynu config validate`
- `contynu config show`
- `contynu mcp-server`

## Known Limitations

- PTY transport is implemented in-process on Unix, but still needs deeper signal/process-group polish
- Interruption handling is best-effort rather than full signal choreography
- Checkpoint packets are deterministic but still evolving in how mission and recent context are selected
- Provider-native rehydration currently targets workspace instruction files (`AGENTS.md`, `CLAUDE.md`, `GEMINI.md`) rather than deeper session-native APIs
- Semantic retrieval remains intentionally deferred

## Next Best Steps

1. Strengthen rehydration packet quality from model-written memories
2. Expand MCP tool coverage for richer memory operations
3. Add richer adapter integration for vendor-specific session APIs
4. Add integration coverage around resume/handoff workflows
