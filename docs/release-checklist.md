# Release Checklist

Use this checklist before cutting a serious Contynu release.

## Runtime

- PTY and pipe transports both pass the full test suite
- interactive launcher smoke coverage is green
- signal/interruption handling is validated for common cases
- workspace context files are restored correctly after runs

## Storage and Recovery

- `cargo test` is green
- SQLite WAL recovery is verified for crash scenarios
- checkpoint generation tests are green
- legacy data cleanup runs correctly on first launch after upgrade

## MCP Server

- All 8 MCP tools respond correctly (`write_memory`, `update_memory`, `delete_memory`, `record_prompt`, `search_memory`, `list_memories`, `suggest_consolidation`, `consolidate_memories`)
- Memory writes via MCP are persisted and searchable
- Prompt recording works with and without interpretation
- MCP server auto-registration works for Claude, Codex, and Gemini
- Startup external session ingestion still pulls in missing Claude/Codex/Gemini session files

## Launcher Layer

- `.contynu/config.json` is seeded correctly by `contynu init`
- `contynu config validate` is green
- known launchers (`codex`, `claude`, `gemini`) have documented defaults
- launcher override behavior is covered by smoke tests

## Rehydration

- Rehydration packets include model instructions for MCP tool usage
- Packets render correctly in XML (Claude), Markdown (Codex), and StructuredText (Gemini)
- Budget-aware memory selection works for large memory stores
- Working-set carry-forward and recent-change layering still produce compact packets
- AI-facing packet output remains sanitized and does not leak source-model provenance

## Product Surface

- `contynu status`
- `contynu projects`
- `contynu recent`
- `contynu doctor`
- `contynu checkpoint`
- `contynu resume`
- `contynu handoff`

All should be exercised against a real local state directory before release.

## Distribution

- GitHub release workflow succeeds for the supported release target set
- release artifacts include installers and checksums
- `scripts/install.sh` installs correctly on Linux and macOS
- `scripts/install.ps1` installs correctly on Windows
- startup self-update check detects newer platform releases and offers exact manual and auto-update flows
- README install instructions match the published release assets

## Documentation

- README reflects actual behavior
- CLI doc reflects actual commands
- crash recovery doc reflects actual recovery semantics
- rehydration doc reflects actual packet construction rules
- adapter architecture doc reflects actual launcher config behavior

## Final Audit

- architecture docs and ADRs still match the implementation
- no dirty worktree remains
- latest commits are coherent and intentional
- known limitations are documented honestly
