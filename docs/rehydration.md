# Rehydration Semantics

Contynu does not treat raw transcript dumping as resume logic. Resume and handoff use a deterministic rehydration packet assembled from model-written memories and recorded prompts.

## Packet Sections

- mission
- stable facts
- constraints
- decisions
- current state
- open loops
- recent prompts
- retrieval guidance

## Sources

- memory objects written by models via MCP tools
- recorded user prompts
- session metadata

## Modes

- `resume`: same-project continuation
- `handoff`: target-model annotated continuation

## Current Behavior

Packets are assembled from:

- active memory objects in SQLite, ranked by importance
- recorded user prompts for recent context
- session metadata for project identity

## Model Instructions

Rehydration packets include explicit instructions telling the model how to use Contynu's MCP tools:
- `write_memory` for recording new facts, decisions, constraints, and todos
- `update_memory` for correcting existing memories
- `delete_memory` for removing stale information
- `record_prompt` for recording user prompts at every stop point

These instructions are rendered in the model's preferred format:
- **XML** for Claude (via `--append-system-prompt`)
- **Markdown** for Codex (via `AGENTS.md`)
- **StructuredText** for Gemini (via `GEMINI.md`)

## Philosophy

The old system derived memories heuristically from captured output. The new system relies on models to write memories directly. This means rehydration packets contain exactly what models decided was worth remembering, not what a parser guessed might be important.
