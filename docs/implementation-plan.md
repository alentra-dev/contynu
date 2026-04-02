# Contynu Implementation Plan

## Audit Summary

The current repository is a credible foundation but not yet a coherent tool. The main gaps found during audit:

- the Rust workspace compiles only partially and currently fails immediately
- event envelope docs, JSON schema, SQLite schema, and Rust types drift from each other
- identifier semantics are inconsistent, especially around turn IDs and checksum fields
- the journal implementation is too thin for durable replay and tail repair
- the SQLite layer is incomplete relative to the documented storage model
- blob storage exists only as a minimal byte writer
- checkpoint and rehydration types exist, but not a real pipeline
- the CLI is still scaffold-grade rather than product-grade

## Delivery Sequence

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

## Architectural Guardrails

- append-only JSONL journal remains the canonical truth
- SQLite remains derived structured state
- blob storage remains content-addressed and local-first
- exact replay and deterministic rehydration come before semantic memory
- adapters stay vendor-neutral and normalized
- expensive enrichment stays off the hot path

## Expected Outcome For This Pass

At the end of this pass the repository should compile, pass tests, provide a real `contynu run -- <command>` path, persist canonical events durably, recover from truncated tails, generate checkpoints and rehydration packets, and expose a coherent local-first CLI and docs baseline.
