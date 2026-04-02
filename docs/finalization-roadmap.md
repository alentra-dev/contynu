# Finalization Roadmap

This document tracks the remaining work required to bring Contynu to a polished first release.

## 1. Runtime Finalization

- Replace the current `script`-based PTY backend with a first-class PTY implementation.
- Improve process-group handling, signal forwarding, and interrupt cleanup.
- Add terminal resize support where practical.
- Expand long-running interactive session coverage.

Done when:
- interactive launchers do not depend on `script`
- interrupt/exit behavior is deterministic under integration tests
- PTY and pipe transports both have stable replayable event output

## 2. Adapter Finalization

- Keep `.contynu/config.json` as the authoritative launcher integration layer.
- Add richer validation and diagnostics for launcher config entries.
- Expand launcher-specific normalization where safe and documented.

Done when:
- known launcher defaults are fully documented
- invalid launcher config fails with actionable errors
- launcher-specific startup mapping is test-covered

## 3. Memory Finalization

- Replace lightweight heuristics with stronger deterministic extraction rules.
- Improve supersession policy across facts, constraints, decisions, and todos.
- Expand provenance and confidence semantics.

Done when:
- rehydration packets are materially better than transcript excerpts
- superseded memory is consistently hidden from active recall
- memory derivation behavior is integration-tested

## 4. File and Artifact Finalization

- Improve file-role classification and MIME inference.
- Add clearer source/generated/artifact policies.
- Tighten deletion and current-file semantics further.

Done when:
- file/artifact metadata is reliable enough for resume and audit
- large and binary outputs are classified consistently

## 5. Rehydration and Resume Finalization

- Improve relevant-file and relevant-artifact selection.
- Tighten packet compactness and stability.
- Add stronger repeated resume/handoff tests.

Done when:
- resume and handoff packets stay compact and high-signal across long projects
- restart and model-switch scenarios have end-to-end coverage

## 6. Journal and Store Hardening

- Expand corruption and reconciliation coverage.
- Add clearer repair reporting and divergence handling.
- Review indexing behavior for larger histories.

Done when:
- repair/reconcile behavior is well specified and tested
- journal/SQLite divergence has a clear recovery path

## 7. CLI and UX Finalization

- Continue polishing `status`, `projects`, `recent`, `doctor`, and inspect/search output.
- Add more human-readable operational views in addition to JSON.
- Add config doctor/validation ergonomics.

Done when:
- the common operator workflows feel product-grade rather than debug-grade

## 8. Documentation Finalization

- Rewrite README as a polished product entry point.
- Expand operator and developer guides.
- Add launcher config examples and recovery walkthroughs.

Done when:
- a new developer can understand setup, runtime behavior, recovery, and customization from repo docs alone

## 9. Testing Finalization

- Add more integration fixtures for PTY, resume/handoff, memory derivation, and repair.
- Add larger session-history test cases.

Done when:
- the critical durability, recovery, and launcher flows are exercised end to end

## 10. Release Readiness

- Audit implementation against architecture docs and ADRs.
- Remove remaining rough compatibility edges.
- Produce a release checklist and versioned release notes.

Done when:
- the repository feels internally coherent and releaseable without hidden caveats
