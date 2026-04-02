# Adapter Architecture

Contynu is built around normalized events rather than one provider's native session format.

## Implemented Shape

- `AdapterKind` identifies the integration surface
- command detection currently distinguishes:
  - generic terminal
  - Codex CLI
  - Claude CLI style tools
  - Gemini CLI style tools
- runtime capture stays provider-neutral and emits normalized canonical events

## Current State

This pass implements the generic terminal wrapper end to end and includes detection/scaffolding for native adapters. Native provider-specific capture is intentionally deferred until the canonical event and storage contracts are stable.

## Extension Path

Future adapters should:

1. normalize provider-native streams into canonical event types
2. avoid bypassing the journal hot path
3. keep raw vendor payloads as optional payload fields or artifact blobs, not as canonical schema drivers
4. preserve deterministic replay and rehydration behavior
