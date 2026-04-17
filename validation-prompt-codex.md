# Contynu Validation Prompt — Codex Adapter

You are being launched through Contynu — a model-agnostic persistent memory layer. Your job is to validate that Contynu is functioning correctly end-to-end. Run through each check below, report PASS/FAIL for each, and summarize at the end.

## 1. Rehydration Check
- Did you receive a rehydration packet or continuity context at the start of this session?
- What format is it in? For Codex, it should be **Markdown** (H1/H2 headers, bullet lists, blockquotes) delivered via **stdin and/or AGENTS.md**.
- Does it contain project identity, memory objects (facts, constraints, decisions), and/or dialogue history?
- Check: does `AGENTS.md` exist in the working directory with the rendered rehydration context?

## 2. Memory Content Validation
- List all memory objects you received. For each, note: kind (fact, constraint, decision, todo, summary), importance, confidence, and text.
- Are there any Fact, Decision, Constraint, Todo, or Summary memories present?
- Do any memories reference cross-model provenance (source_adapter, source_model)? For example, memories originally from `claude_cli` sessions should show that provenance.

## 3. Environment Variables Check
- Codex receives hydration via environment variables. Check if these are set:
  - `CONTYNU_PROJECT_ID`
  - `CONTYNU_REHYDRATION_PACKET_FILE`
  - `CONTYNU_REHYDRATION_PROMPT_FILE`
  - `CONTYNU_REHYDRATION_SCHEMA_VERSION`
- Report the values (or note if any are missing).

## 4. CLI Tools Check
- Codex does NOT have MCP tool access. Instead, use the CLI directly.
- Run: `contynu search-memory --query "Contynu" --limit 5` (or the equivalent CLI subcommand)
- Run: `contynu status`
- Run: `contynu doctor`
- Report whether each command responds correctly.

## 5. Version & State Check
- Confirm the version is 0.4.0 by checking `Cargo.toml` in the repo, or `contynu --help` output.
- From `contynu status`, confirm the project is active with a non-zero event count.

## 6. Checkpoint Test
- Run: `contynu checkpoint --reason "codex validation test"`
- Confirm it succeeds and produces a checkpoint ID.
- List the contents of the checkpoint directory to verify it contains `manifest.json` and `rehydration.json`.

## 7. Cross-Adapter Provenance Check
- This session was preceded by a Claude CLI session that ran a similar validation.
- Check your rehydration packet or memory objects for any evidence of that prior Claude CLI session (e.g., facts with `source_adapter: claude_cli`, or dialogue referencing the prior validation).
- This tests whether Contynu correctly transfers context across different model adapters.

## 8. AGENTS.md Integrity Check
- Read the `AGENTS.md` file in the working directory.
- Confirm it contains structured rehydration content (not empty, not stale).
- Confirm the project ID in `AGENTS.md` matches what `contynu status` reports.

## Notes on Codex-specific behavior
- Codex receives rehydration as **Markdown via stdin + AGENTS.md file**, not XML in system prompt.
- Codex does **not** have MCP tool access — all queries must go through the CLI.
- Progressive loading tiers (L0/L1/L2) are delivered all-at-once in the stdin payload, not on-demand.

---

Report a summary table of all checks with PASS/FAIL status. If everything passes, Contynu's Codex adapter is working end-to-end.
