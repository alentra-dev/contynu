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

This pass implements the generic terminal wrapper end to end and includes a first hydration delivery layer for known LLM launchers. Contynu now:

- detects known launcher commands
- builds a normalized rehydration packet when continuing an existing project
- materializes packet and prompt files under `.contynu/runtime/<project-id>/`
- passes those file paths through environment variables
- sends a startup prelude on stdin

Native provider-specific argument-level integration is still deferred until the canonical event and storage contracts are stable.

## Configurable Launchers

Contynu can be taught about future LLM tools through `.contynu/config.json`.

Example:

```json
{
  "llm_launchers": [
    {
      "command": "futurellm",
      "aliases": ["futurellm-cli"],
      "hydrate": true,
      "extra_env": {
        "FUTURELLM_MODE": "enabled"
      }
    }
  ]
}
```

If a direct launch command matches `command` or any alias, Contynu treats it as a hydratable LLM launcher and applies the same rehydration delivery path used by built-in launchers.

## Extension Path

Future adapters should:

1. normalize provider-native streams into canonical event types
2. avoid bypassing the journal hot path
3. keep raw vendor payloads as optional payload fields or artifact blobs, not as canonical schema drivers
4. preserve deterministic replay and rehydration behavior
