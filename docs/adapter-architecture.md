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
- can install a temporary provider-native workspace context file
- passes those file paths through environment variables
- can prepend adapter-specific hydration arguments
- can send a startup prelude on stdin
- can switch configured launchers onto PTY transport when requested

## Configurable Launchers

Contynu can be taught about future LLM tools through `.contynu/config.json`.
That same config file is also the preferred override layer for the known launchers that Contynu seeds during `init`, so launcher-specific startup parameters can evolve without waiting for a new Contynu release.

Example:

```json
{
  "llm_launchers": [
    {
      "command": "futurellm",
      "aliases": ["futurellm-cli"],
      "hydrate": true,
      "use_pty": true,
      "context_file": "FUTURELLM.md",
      "hydration_delivery": "env_only",
      "hydration_args": ["--context-file", "{prompt_file}", "--project", "{project_id}"],
      "extra_env": {
        "FUTURELLM_MODE": "enabled"
      }
    }
  ]
}
```

If a direct launch command matches `command` or any alias, Contynu treats it as a hydratable LLM launcher.

Configured launchers can choose how rehydration is delivered:

- `env_only`: materialize packet/prompt files and export their paths via environment variables only
- `stdin_only`: send the startup prelude on stdin only
- `env_and_stdin`: do both
- `use_pty`: run the launcher under PTY transport when available
- `context_file`: install a temporary workspace instruction file for the launcher during the run and restore the original file afterward

Configured launchers can also define `hydration_args`, which are prepended to the launcher command when hydration is active. Supported placeholders are:

- `{prompt_file}`
- `{packet_file}`
- `{project_id}`
- `{schema_version}`

## Extension Path

Future adapters should:

1. normalize provider-native streams into canonical event types
2. avoid bypassing the journal hot path
3. keep raw vendor payloads as optional payload fields or artifact blobs, not as canonical schema drivers
4. preserve deterministic replay and rehydration behavior
