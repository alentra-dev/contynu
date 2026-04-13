# Rehydration Semantics

Contynu does not treat raw transcript dumping as resume logic. Resume and handoff use a deterministic rehydration packet assembled from model-written memories and recorded prompts.

## Packet Sections

- mission
- current state
- recent changes
- constraints
- decisions
- open loops
- durable context
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

- active memory objects in SQLite, ranked by blended relevance rather than raw kind buckets
- the working set carried forward from previous successful packets
- startup-ingested external session memory from Claude, Codex, and Gemini when present
- recorded user prompts for recent context
- session metadata for project identity

## Selection Rules

Contynu now tries to make rehydration feel invisible instead of document-like. Packet construction uses:

- a real configurable packet budget
- lexical relevance to the latest prompt
- model-assigned importance
- recency
- access frequency
- scope bias
- working-set boosts for memories that were recently useful

The result is a layered packet that favors "what matters now" over broad memory taxonomy coverage.

## First Run Behavior

When a project has no prior Contynu memory yet, packets explicitly mark startup mode instead of pretending there is continuity. This lets the model treat the repo and the user's next request as ground truth while still beginning to write durable memory correctly.

## Model Instructions

Rehydration packets include explicit instructions telling the model how to use Contynu's MCP tools:
- `write_memory` for recording new facts, decisions, constraints, and todos
- `update_memory` for correcting existing memories
- `delete_memory` for removing stale information
- `record_prompt` for recording user prompts at every stop point

These instructions are rendered in the model's preferred format:
- **XML** for Claude (via `--append-system-prompt`)
- **Markdown** for Codex (via an `AGENTS.md`-first working continuation block)
- **StructuredText** for Gemini (via `GEMINI.md`)

## Observability and Hygiene

Each packet generation pass now records packet observations so Contynu can explain which memories were selected and why. Checkpoint generation also runs lightweight Dream Phase candidate detection and stores hygiene pressure signals without automatically mutating the archive.

## Philosophy

The old system derived memories heuristically from captured output. The new system relies on models to write memories directly. This means rehydration packets contain exactly what models decided was worth remembering, not what a parser guessed might be important.
