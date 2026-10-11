# MCP servers

rust-coding-agent connects to [Model Context Protocol](https://modelcontextprotocol.io) servers over stdio or Streamable HTTP and gives their tools to the model. Add servers to `~/.rust-coding-agent/mcp.json`, or to `.rust-coding-agent/mcp.json` in a project. The format uses the standard `mcpServers` shape:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]
    },
    "docs": {
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer ${DOCS_TOKEN}" }
    }
  }
}
```

- `command` is a single executable and `args` its arguments. `env` sets environment variables and `cwd` the working folder. A leading `~/` in `command`, an argument or `cwd` names the home folder.
- `url` is the address of a Streamable HTTP server and must start with `http://` or `https://`. `headers` sets extra request headers, such as `Authorization`. `${NAME}` in a header value is replaced with the environment variable `NAME`; the server fails to connect if it is not set. A project `mcp.json` can use this too, so it can send your environment variables to a server it chooses. If connecting fails because the server cannot be reached or answers with status 408, 429 or 5xx (other than 501), rust-coding-agent tries twice more, after 0.25 and 1 second. rust-coding-agent keeps the session the server gives it and ends the session when it quits. If the server says the session has expired (status 404), rust-coding-agent starts a new one and sends the request once more. A server without an `Authorization` header signs in with OAuth; see [Signing in to MCP servers](#signing-in-to-mcp-servers).
- `oauth` sets how rust-coding-agent signs in to an HTTP server. `clientId` and `clientSecret` name a client you registered yourself, so rust-coding-agent does not register; `${NAME}` in `clientSecret` is replaced with the environment variable `NAME`. rust-coding-agent reads `clientSecret` from `mcp.json` each time it starts the server and does not save it in `mcp-auth.json`.
- `timeout` sets the time limit for each request in seconds, at least 1 (default 60). When a request times out, or you press Esc during a tool call, rust-coding-agent tells the server to stop working on it.
- `enabled: false` keeps an entry without connecting to it.
- `type` is optional. When present, it must be `stdio`, `http` or `streamable-http`. Without `type`, an entry with `url` and no `command` is an HTTP server. The older SSE transport (`sse`) is not supported; use the server's Streamable HTTP URL.
- Server names may only contain letters, digits, `_` and `-`.

Project entries replace global entries with the same name. rust-coding-agent starts project servers without asking (see [ADR 3](adr/0003-start-project-mcp-servers-without-approval.md)). When you run rust-coding-agent from your home folder, `~/.rust-coding-agent/mcp.json` counts as global only. A missing `mcp.json` is fine; one that exists but cannot be read gives a notice.

In the interface, rust-coding-agent starts all servers in the background, so you can type and send prompts straight away. It lists the servers on two lines, one for global servers and one for project servers, with a spinner on each server still loading, such as `project MCP servers: ⠋ docs, web (2 tools)`. Each server's tools become available once it is ready, and its entry then changes to its tool count, such as `docs (3 tools)`, or to `docs (failed)` or `docs (needs sign-in)`. Once every server on a line is ready, the line starts with `loaded`, such as `loaded project MCP servers: docs (3 tools), web (2 tools)`. A global server that a project entry overrides stays on the global line as `docs (overridden)`. The reason a server failed shows on its own line. A prompt sent before then goes without those tools, and a server that becomes ready while a prompt runs joins after that prompt. The interface also shows a notice for each invalid entry. In print mode, rust-coding-agent waits for all servers before sending the prompt, and the notices for invalid entries and failed servers go to standard error. Tools are named `mcp__<server>__<tool>`, with other characters replaced by `_` and cut to 64 characters. If two tools end up with the same name, the one loaded first is kept and a notice names the tool that was skipped. `anyOf`, `oneOf` and `allOf` at the top level of a tool's input schema are dropped, because the API rejects them. Text results longer than 20,000 bytes are cut. Images, audio and binary resources show as short placeholders. Servers stop when rust-coding-agent quits.

## Signing in to MCP servers

An HTTP server without an `Authorization` header in its entry signs in with OAuth. When it needs sign-in, its entry shows `figma (needs sign-in)`. Run `/mcp login figma` to sign in.

Type `/mcp login <server>` in the interface. After `/mcp `, the command list offers `login` and `logout`, and after either one the names of the HTTP servers that sign in with OAuth. rust-coding-agent finds the server's sign-in server, registers with it unless `oauth.clientId` is set, and opens the sign-in page in your browser (`open` on macOS, `xdg-open` elsewhere). It also shows the link, in case the browser does not open. It refuses to sign in when the MCP server's URL, or the sign-in server's sign-in page or token address, does not start with `https://`, other than `http://` addresses on this computer, such as `localhost` or `127.0.0.1`. It also refuses sign-in details that name another server as the `resource` (the server's URL, its origin or a parent path are fine), or a sign-in server whose metadata names another `issuer`, so a server cannot get a sign-in meant for another service. The page sends the browser back to `http://127.0.0.1:<port>/callback`, where rust-coding-agent waits for up to 5 minutes. rust-coding-agent then starts the server again with the new sign-in, and its entry shows the spinner again. You can keep working while it waits.

rust-coding-agent saves the sign-in for each server URL in `~/.rust-coding-agent/mcp-auth.json`, which only you can read. It renews the sign-in when it expires or the server turns it down. `/mcp logout <server>` removes the saved sign-in and starts the server again without it. In print mode, servers use the saved sign-in, but you can only sign in from the interface.

rust-coding-agent registers as `Claude Code`, because some sign-in servers only let approved apps register. Figma, for example, turns down `rust-coding-agent` but accepts `Claude Code`. Using another app's name may break the server's terms (see [ADR 1](adr/0001-identify-as-claude-code.md)).
