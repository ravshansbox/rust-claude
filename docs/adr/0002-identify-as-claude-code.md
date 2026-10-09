# 2. Identify as Claude Code

Date: 2026-10-09

## Status

Accepted

## Context

rust-claude signs in with a Claude Pro or Max account instead of a paid API key. Identifying as Claude Code is the only way to use the subscription this way.

## Decision

rust-claude signs in with Claude Code's OAuth client ID (`src/auth.rs`) and starts the system prompt with Claude Code's identity line (`src/agent.rs`).

We accept the risk to the account and the risk of breaking Anthropic's terms.

## Consequences

- rust-claude uses the subscription quota, not paid API usage.
- This may break Anthropic's terms, and Anthropic may limit or close the account.
- Anthropic may change the client ID, scopes or checks at any time, which can stop sign-in or requests from working.
