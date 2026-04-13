# Contynu Implementation Plan (Historical)

> **This document is historical.** It describes the original implementation plan for Contynu's v0.1.0-v0.4.0 journal-based architecture. That architecture has been superseded by the v0.5.0 model-driven memory rewrite.

## Original Audit Summary

The original plan addressed gaps in the early codebase: partial compilation, schema drift, thin journal implementation, incomplete SQLite layer, minimal blob storage, scaffold-grade CLI.

## Original Delivery Sequence

1. Normalize the workspace so `cargo test` runs cleanly.
2. Stabilize core IDs, event envelope semantics, canonical serialization, and validation.
3. Harden the append-only journal with checksum verification, replay, and truncated-tail recovery.
4. Bring the SQLite metadata layer in line with the storage model and add reconciliation from journal to SQLite.
5. Finish the content-addressed blob store with integrity checks and deduplication.
6. Implement checkpoint creation and deterministic rehydration packet generation.
7. Add the runtime wrapper and adapter scaffolding around external commands.
8. Expand the CLI surface to a coherent operational interface.
9. Add unit and integration coverage for the hot path and recovery behavior.
10. Update README and implementation docs to match reality.

All items were completed through v0.4.0.

## What Changed in v0.5.0

The v0.5.0 rewrite removed the JSONL journal, event sourcing pipeline, heuristic memory derivation, file tracking, and artifact management. It replaced them with a model-driven memory architecture where models write memories directly via MCP tools.

See [`architecture.md`](architecture.md) for the current system design.
