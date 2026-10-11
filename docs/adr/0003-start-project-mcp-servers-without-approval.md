# 3. Start project MCP servers without approval

Date: 2026-10-09

## Status

Accepted

## Context

A project can list MCP servers in `.rust-coding-agent/mcp.json`. Stdio servers are commands, so opening a project with this file runs them.

## Decision

rust-coding-agent reads `.rust-coding-agent/mcp.json` in the current directory and starts its servers without asking, in line with [ADR 2](0002-run-tools-without-approval.md).

## Consequences

- Project servers work with no extra step.
- Starting rust-coding-agent in a project runs any command its `mcp.json` lists, before the first prompt.
- As with ADR 2, use rust-coding-agent only in projects you trust.
