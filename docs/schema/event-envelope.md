# Canonical Event Envelope

Every raw journal record in Contynu is represented as a single canonical event envelope.

## Goals

- immutable and append-only
- deterministic replay
- cryptographically verifiable payload integrity
- independent of any single LLM provider
- stable across runtime and schema evolution

## Envelope

```json
{
  "schema_version": 1,
  "event_id": "evt_018f4fe0d6e24d85a7c718b9698cdb51",
  "session_id": "ses_6f65b6d487e145e2a043f0f7f7f0fdd9",
  "turn_id": "trn_6b10f6b9c8ab4d8b9b7223de8cb193d8",
  "seq": 42,
  "ts": "2026-04-02T05:30:00Z",
  "actor": "assistant",
  "event_type": "message_output",
  "parent_event_id": "evt_018f4fe0d6e24d85a7c718b9698cdb50",
  "payload": {},
  "payload_sha256": "…"
}
```

## Required fields

- `schema_version`: integer contract version for the envelope itself
- `event_id`: globally unique event identifier
- `session_id`: parent session identifier
- `turn_id`: parent turn identifier when applicable
- `seq`: monotonically increasing per-session sequence number
- `ts`: RFC 3339 UTC timestamp
- `actor`: one of `user`, `assistant`, `tool`, `system`, `runtime`
- `event_type`: normalized event category
- `payload`: event-specific body
- `payload_sha256`: SHA-256 digest of the canonical JSON serialization of `payload`

## Optional fields

- `parent_event_id`: direct causal predecessor for chained events

## Event types

Minimum supported event classes:

- `session_started`
- `session_ended`
- `session_interrupted`
- `session_resumed`
- `turn_started`
- `turn_completed`
- `message_input`
- `message_output`
- `tool_call`
- `tool_result`
- `file_snapshot`
- `file_diff`
- `artifact_created`
- `artifact_read`
- `checkpoint_created`

## Event ID strategy

IDs are prefixed by object class and use UUID v4 without dashes by default.

Examples:
- `evt_018f4fe0d6e24d85a7c718b9698cdb51`
- `ses_6f65b6d487e145e2a043f0f7f7f0fdd9`
- `trn_6b10f6b9c8ab4d8b9b7223de8cb193d8`

This keeps IDs:
- locally generatable
- stable
- URL-safe
- easy to classify in logs and indexes

## Payload canonicalization

The payload hash is computed from a deterministic serialization of the payload object:

- UTF-8 JSON
- stable key ordering
- no insignificant whitespace

The envelope itself is not hashed as the canonical integrity target; the payload is. Journal tamper detection can additionally be layered through record chaining in future versions.

## Compatibility rules

- new required envelope fields require a `schema_version` increment
- new payload fields must be backward-compatible wherever possible
- readers must reject events with missing required fields
- readers may ignore unknown payload fields
