# 5. Accept only the listed models

Date: 2026-10-11

## Status

Accepted. Supersedes [ADR 4](0004-offer-only-the-latest-model-in-each-class.md).

## Context

ADR 4 let older and dated models through `/model <id>` and `--model <id>`, but rust-coding-agent treated them as unknown. They had no context window, so automatic compaction never ran, and replies were limited to 8,192 tokens. The `/model` picker also had to ask the API for the model list.

## Decision

rust-coding-agent accepts only the four models it lists: `claude-fable-5-1`, `claude-opus-5-5`, `claude-sonnet-5-5` and `claude-haiku-5-5`.

- `--model` with any other id fails before signing in.
- `/model <id>` with any other id shows an error and keeps the typed text.
- A saved `model` setting with any other id shows a warning and uses `claude-opus-5-5`, so a setting saved by an older version does not stop rust-coding-agent from starting.
- The `/model` picker shows the four models without asking the API.

## Consequences

- Every model in use has a known context window, output limit and thinking mode.
- The `/model` picker opens at once and works offline.
- Older and dated models cannot be used.
- Each new model release means updating the list.
