# contynu-openclaw

Permanent memory for [OpenClaw](https://github.com/openclaw/openclaw) agents powered by [Contynu](https://contynu.com).

## What It Does

When OpenClaw agents switch models or when context gets compacted, they lose everything. This plugin gives them permanent memory that survives compaction, model switches, and session boundaries.

- **Records every user prompt** — verbatim via OpenClaw's `afterTurn()` lifecycle hook
- **Writes meaningful memories** — assistant content stored as project knowledge via MCP tools
- **Protects against compaction loss** — checkpoints before compaction fires, writes importance-ranked facts back to MEMORY.md
- **MCP tools for memory management** — agents can write, search, update, and delete memories via `write_memory`, `search_memory`, `update_memory`, `delete_memory`, `list_memories`, and `record_prompt`
- **Per-agent memory isolation** — each agent gets its own memory store
- **Works with any model** — Anthropic, OpenAI, Google, Llama, Mistral, DeepSeek, Ollama

## Install

Requires the `contynu` binary installed on your system:

```bash
curl -fsSL https://github.com/alentra-dev/contynu/releases/latest/download/install.sh | sh
```

Then set up the integration:

```bash
contynu openclaw setup
```

Install the plugin:

```bash
openclaw plugins install contynu-openclaw --dangerously-force-unsafe-install
openclaw plugins enable contynu-openclaw
```

Restart OpenClaw. Every agent gets permanent memory automatically.

## How It Works

1. **afterTurn()** — After each conversation turn, the plugin records user prompts verbatim and writes assistant content as project knowledge via Contynu's MCP tools
2. **session:compact:before** — Before OpenClaw compacts the context window, the plugin creates a Contynu checkpoint and writes the most important memories back to `MEMORY.md` using marker-delimited sections
3. **MCP server** — Registered automatically in OpenClaw's config. Agents can call `write_memory`, `search_memory`, `update_memory`, `delete_memory`, and `list_memories` to manage their own memory directly

## Configuration

Add to your OpenClaw config (`~/.openclaw/openclaw.json`):

```json
{
  "plugins": {
    "contynu-openclaw": { "enabled": true }
  }
}
```

Optional plugin config:

```json
{
  "plugins": {
    "entries": {
      "contynu-openclaw": {
        "enabled": true,
        "config": {
          "stateDir": ".contynu",
          "maxMemoryChars": 18000
        }
      }
    }
  }
}
```

## License

MPL-2.0

## Links

- [Contynu website](https://contynu.com)
- [Contynu GitHub](https://github.com/alentra-dev/contynu)
- [Technical overview](https://github.com/alentra-dev/contynu/blob/main/docs/contynu-technical-overview.md)
