# Canonical Event Model (Superseded)

> **This specification is obsolete.** The JSONL journal and event sourcing pipeline were removed in the v0.5.0 architecture rewrite. Contynu no longer captures, stores, or replays events. The canonical event envelope, 37 event types, checksums, sequence numbers, and deterministic replay are all gone.

## What Replaced It

Contynu v0.5.0 uses a **model-driven memory architecture**. Instead of capturing every interaction as immutable events and deriving memory heuristically, models now write structured memories directly via MCP tools.

### MCP Memory Tools

| Tool | Purpose |
|------|---------|
| `write_memory` | Create a new memory (fact, decision, constraint, todo, user_fact, project_knowledge) |
| `update_memory` | Update an existing memory's text, importance, or reason |
| `delete_memory` | Remove a memory that is no longer relevant |
| `record_prompt` | Record the user's prompt verbatim with optional interpretation |
| `search_memory` | Search memories by text, kind, scope, or time window |
| `list_memories` | Browse all memories with filtering and sorting |

### Memory Object Format

```json
{
  "memory_id": "mem_019d...",
  "session_id": "prj_019d...",
  "kind": "decision",
  "scope": "project",
  "status": "active",
  "text": "Use HMAC-SHA256 for token signing",
  "importance": 0.85,
  "reason": "Security architecture decision made during auth design",
  "source_model": "claude-sonnet-4-20250514",
  "created_at": "2026-04-13T10:00:00Z"
}
```

### Memory Kinds

- `fact` — something true about the project
- `constraint` — a rule or limitation
- `decision` — a choice that was made
- `todo` — an unfinished task
- `user_fact` — a fact about the user (persists across projects)
- `project_knowledge` — broader project context

### Memory Scopes

- `user` — persists across all projects
- `project` — this project only (default)
- `session` — this session only

### Prompt Record Format

```json
{
  "prompt_id": "pmt_019d...",
  "session_id": "prj_019d...",
  "verbatim": "Fix the auth bug in the login endpoint",
  "interpretation": "Fix JWT validation error in POST /api/login",
  "interpretation_confidence": 0.9,
  "source_model": "claude-sonnet-4-20250514",
  "created_at": "2026-04-13T10:00:00Z"
}
```

### Key Philosophy Change

**Old system (v0.4.0):** Contynu watches model output -> heuristic parser guesses what matters -> stores everything at flat importance -> rehydrates noise.

**New system (v0.5.0):** Model generates response -> model writes structured memories via MCP -> Contynu stores them verbatim -> rehydrates signal. User prompts are always recorded. No heuristics, no Jaccard dedup, no confidence scoring by Contynu.

See [`architecture.md`](../architecture.md) for the current system design.
