# 2. Run tools without approval

Date: 2026-10-09

## Status

Accepted

## Context

The agent can use `bash`, `read`, `write` and `edit`. Asking the user to approve each call slows down every task and adds friction to the interface.

## Decision

rust-claude runs every tool call straight away, without asking for approval and without limiting which paths a tool can use.

Use rust-claude only in projects you trust and that are under version control.

## Consequences

- Tasks run faster, with no approval prompts.
- The agent can run any shell command and change or delete any file the user can reach, including files outside the project.
- A wrong or harmful command runs before the user can stop it. Version control is the main way to undo changes in the project. Changes outside the project may not be recoverable.
