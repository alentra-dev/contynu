# Canonical Event Model

## Purpose

The canonical event model defines the immutable units of truth for Contynu. Every captured interaction, artifact transition, and recovery checkpoint must be representable as one or more normalized events.

This model is the foundation for:
- deterministic replay
- exact auditability
- derived memory generation
- checkpointing
- cross-model handoff
- future sync/export

---

## Design Goals

1. **Append-only**
   Events are never mutated after durable commit.

2. **Atomic**
   Each event should represent one meaningful state transition or observed action.

3. **Composable**
   Higher-level behavior should emerge from ordered event sequences.

4. **Portable**
   The event model must remain independent of any one storage backend or vendor-specific CLI.

5. **Versioned**
   The envelope and payloads must support forward evolution without ambiguity.

---

## Canonical Event Envelope

Every event must conform to this envelope.

```json
{
  "schema_version": 1,
  "event_id": "evt_01J...",
  "session_id": "ses_01J...",
  "turn_id": "turn_01J...",
  "seq": 42,
  "ts": "2026-04-03T01:23:45.123456Z",
  "actor": "assistant",
  "event_type": "message_output",
  "payload_version": 1,
  "payload": {},
  "checksum": "sha256:...",
  "parent_event_id": "evt_01J...",
  "correlation_id": "corr_01J...",
  "causation_id": "evt_01J...",
  "tags": ["terminal", "stdout"]
}
```

---

## Envelope Fields

### `schema_version`
Version of the event envelope schema.

### `event_id`
Globally unique identifier for the event.

Requirements:
- string
- immutable
- unique across all sessions
- monotonic ordering is desirable but not required if `seq` is authoritative per session

Recommended format:
- ULID or UUIDv7

### `session_id`
Identifier for the owning session.

### `turn_id`
Identifier for the turn associated with the event. Some lifecycle events may not belong to a user turn; in those cases, Contynu may use a reserved system turn.

### `seq`
Monotonic per-session sequence number.

Requirements:
- strictly increasing within a session
- canonical replay order field
- assigned at durable write time

### `ts`
UTC timestamp with sub-second precision.

### `actor`
Actor responsible for the event.

Allowed initial values:
- `system`
- `user`
- `assistant`
- `tool`
- `runtime`
- `filesystem`
- `adapter`

### `event_type`
Normalized event classification.

### `payload_version`
Version of the event-type-specific payload schema.

### `payload`
Structured event body specific to the event type.

### `checksum`
Integrity hash of the canonical serialized event content.

Requirements:
- computed after sequence assignment
- excludes `checksum` field itself from digest input
- recommended format: `sha256:<hex>`

### `parent_event_id`
Optional structural parent pointer.

Use cases:
- chunk events under a root event
- file diff events under a file snapshot root
- tool result under tool call

### `correlation_id`
Optional ID linking events that belong to one logical operation.

Use cases:
- one model response streamed as many chunks
- one artifact generation pipeline
- one checkpoint build sequence

### `causation_id`
Optional pointer to the event that directly caused this event.

### `tags`
Optional freeform tags for indexing and filtering.

---

## Core Event Types

### Session Lifecycle
- `session_started`
- `session_interrupted`
- `session_resumed`
- `session_ended`
- `adapter_attached`
- `adapter_detached`

### Turn Lifecycle
- `turn_started`
- `turn_completed`
- `turn_failed`
- `turn_cancelled`

### Message Flow
- `message_input`
- `message_output`
- `message_chunk`
- `message_redaction`

### Tooling
- `tool_call`
- `tool_result`
- `tool_stream`
- `tool_error`

### Terminal / Runtime IO
- `stdin_captured`
- `stdout_captured`
- `stderr_captured`
- `process_started`
- `process_exited`

### File and Workspace
- `file_observed`
- `file_snapshot`
- `file_diff`
- `file_deleted`
- `workspace_scan_completed`

### Artifacts
- `artifact_registered`
- `artifact_materialized`
- `artifact_read`
- `artifact_deleted`

### Recovery and Memory
- `checkpoint_created`
- `rehydration_packet_created`
- `memory_object_derived`
- `memory_object_superseded`

---

## Payload Contracts

### `message_input`
```json
{
  "content": [
    {"type": "text", "text": "Refactor the journal writer."}
  ],
  "attachments": ["art_01J..."],
  "source": "terminal"
}
```

### `message_output`
```json
{
  "content": [
    {"type": "text", "text": "I refactored the journal writer."}
  ],
  "finish_reason": "completed",
  "model": {
    "provider": "openai",
    "name": "gpt-5.4"
  }
}
```

### `tool_call`
```json
{
  "tool_name": "shell",
  "arguments": {
    "cmd": ["cargo", "test"]
  },
  "invocation_source": "assistant"
}
```

### `tool_result`
```json
{
  "tool_name": "shell",
  "status": "ok",
  "output_ref": "art_01J...",
  "exit_code": 0
}
```

### `file_snapshot`
```json
{
  "path": "src/journal/mod.rs",
  "workspace_relative_path": "src/journal/mod.rs",
  "snapshot_kind": "full",
  "content_ref": "blob_01J...",
  "sha256": "...",
  "size_bytes": 2193,
  "mime_type": "text/rust"
}
```

### `file_diff`
```json
{
  "path": "src/journal/mod.rs",
  "base_sha256": "...",
  "head_sha256": "...",
  "diff_ref": "blob_01J...",
  "diff_format": "unified"
}
```

### `artifact_registered`
```json
{
  "artifact_id": "art_01J...",
  "path": "reports/summary.pdf",
  "kind": "output_file",
  "mime_type": "application/pdf",
  "sha256": "...",
  "blob_ref": "blob_01J..."
}
```

### `checkpoint_created`
```json
{
  "checkpoint_id": "chk_01J...",
  "last_seq": 204,
  "rehydration_packet_ref": "blob_01J...",
  "reason": "turn_completed"
}
```

---

## Serialization Rules

1. Canonical serialization for checksumming must use stable key ordering.
2. Timestamps must be UTC ISO-8601 with microsecond precision where available.
3. Unknown fields must be ignored by readers but preserved by transport and export layers where feasible.
4. Payload evolution must use `payload_version`, not ad hoc shape changes.

---

## Compatibility Rules

### Backward Compatibility
- Readers must tolerate known older schema versions.
- Additive fields are preferred over destructive changes.

### Breaking Changes
Any breaking change to envelope semantics requires a new `schema_version` and migration policy.

---

## Validation Rules

An event is invalid if:
- `event_id` is missing
- `session_id` is missing
- `seq` is not strictly increasing within the session
- `event_type` is unknown to the active runtime without an extension namespace policy
- `checksum` does not match canonical content

---

## Extension Policy

Vendor- or adapter-specific payload data must live under clearly namespaced fields inside payloads.

Example:
```json
{
  "provider_extensions": {
    "openai": {
      "response_id": "resp_..."
    }
  }
}
```

This preserves the normalized cross-vendor contract while allowing richer integrations.

---

## Summary

The canonical event model is the immutable contract at the center of Contynu. Everything else—search, summaries, rehydration, sync, and collaboration—must derive from this layer rather than bypass it.
