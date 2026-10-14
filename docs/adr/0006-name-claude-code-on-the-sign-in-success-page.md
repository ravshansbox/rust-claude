# 6. Name Claude Code on the sign-in success page

Date: 2026-10-14

## Status

Accepted

## Context

rust-coding-agent registers with MCP sign-in servers as `Claude Code` (see [ADR 1](0001-identify-as-claude-code.md)), so the server's sign-in page names Claude Code. After sign-in, the browser goes back to a page that rust-coding-agent serves itself. That page said "go back to rust-coding-agent", so the two pages named different apps.

## Decision

The sign-in success page says "Signed in. You can close this tab and go back to Claude Code." (`src/mcp/oauth.rs`).

## Consequences

- The sign-in server and the success page name the same app.
- The success page names an app that is not the one running.
- If ADR 1 changes, the success page must change with it.
