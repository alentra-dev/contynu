# Rehydration Semantics

Contynu does not treat raw transcript dumping as resume logic. Resume and handoff use a deterministic rehydration packet derived from canonical and structured project state.

## Packet Sections

- mission
- stable facts
- constraints
- decisions
- current state
- open loops
- relevant artifacts
- relevant files
- recent verbatim context
- retrieval guidance

## Sources

- journal-backed event index for exact recent context
- structured memory objects for stable facts, constraints, decisions, and todos
- artifact registry for file outputs and binary references

## Modes

- `resume`: same-project continuation
- `handoff`: target-model annotated continuation

## Current Behavior

In this pass, packets are derived from:

- the earliest captured user message for mission inference
- active memory objects in SQLite for reusable state
- recent message and IO events for verbatim context
- tracked artifacts and the structured current-file index for relevant references
