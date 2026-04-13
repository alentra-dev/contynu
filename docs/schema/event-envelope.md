# Canonical Event Envelope (Superseded)

> **This specification is obsolete.** The JSONL journal, event envelope, checksums, sequence numbers, and all 37 event types were removed in the v0.5.0 architecture rewrite. Contynu no longer uses event sourcing.

## What Replaced It

Contynu v0.5.0 uses a **model-driven memory architecture** where models write structured memories directly via MCP tools (`write_memory`, `update_memory`, `delete_memory`, `record_prompt`). There is no event envelope, no journal, and no replay.

See [`../specs/event-model.md`](../specs/event-model.md) for the new memory format, or [`../architecture.md`](../architecture.md) for the current system design.
