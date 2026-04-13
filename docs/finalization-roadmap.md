# Finalization Roadmap

This document tracks the remaining work required to bring Contynu to a polished release after the v0.5.0 model-driven memory rewrite.

## 1. Runtime Finalization

- Improve process-group handling, signal forwarding, and interrupt cleanup.
- Add terminal resize support where practical.
- Expand long-running interactive session coverage.

Done when:
- interactive launchers run reliably under PTY transport
- interrupt/exit behavior is deterministic under integration tests

## 2. Adapter Finalization

- Keep `.contynu/config.json` as the authoritative launcher integration layer.
- Add richer validation and diagnostics for launcher config entries.
- Expand launcher-specific normalization where safe and documented.

Done when:
- known launcher defaults are fully documented
- invalid launcher config fails with actionable errors
- launcher-specific startup mapping is test-covered

## 3. Memory and MCP Finalization

- Ensure model instructions in rehydration packets are clear and effective across Claude, Codex, and Gemini.
- Validate that models use write_memory, update_memory, delete_memory, and record_prompt correctly.
- Expand MCP tool coverage if needed (e.g., bulk operations, memory tagging).

Done when:
- rehydration packets produce high-quality memory writes from all supported models
- memory accumulation stays clean across multi-session projects
- MCP tool behavior is integration-tested

## 4. Rehydration and Resume Finalization

- Improve packet compactness and stability for long-running projects.
- Add stronger repeated resume/handoff tests.
- Tune budget-aware memory selection.

Done when:
- resume and handoff packets stay compact and high-signal across long projects
- restart and model-switch scenarios have end-to-end coverage

## 5. Storage Hardening

- Review SQLite WAL recovery behavior under edge cases.
- Add clearer health reporting via `contynu doctor`.
- Review indexing behavior for larger memory stores.

Done when:
- recovery behavior is well specified and tested
- large memory stores perform well under search and list operations

## 6. CLI and UX Finalization

- Continue polishing `status`, `projects`, `recent`, `doctor`, and inspect output.
- Add more human-readable operational views in addition to JSON.
- Add config doctor/validation ergonomics.

Done when:
- the common operator workflows feel product-grade rather than debug-grade

## 7. Documentation Finalization

- Rewrite README as a polished product entry point.
- Expand operator and developer guides.
- Add launcher config examples and recovery walkthroughs.

Done when:
- a new developer can understand setup, runtime behavior, recovery, and customization from repo docs alone

## 8. Testing Finalization

- Add integration fixtures for PTY, resume/handoff, MCP tool flows, and recovery.
- Add larger memory store test cases.

Done when:
- the critical durability, recovery, and launcher flows are exercised end to end

## 9. Release Readiness

- Audit implementation against architecture docs and ADRs.
- Remove remaining rough compatibility edges.
- Produce a release checklist and versioned release notes.

Done when:
- the repository feels internally coherent and releaseable without hidden caveats
