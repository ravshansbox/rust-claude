# Tools

The agent can use these tools:

- `bash`: run a shell command. An optional `timeout` sets the seconds before the command is stopped, with all its background processes. Without it there is no limit
- `read`: read a file. It reads only up to the requested lines and stops once it has 20,000 bytes of output, so the first lines of a huge file come back straight away. A PNG, JPEG, GIF, WebP or TIFF file is sent to the model as an image instead, prepared the same way as a pasted image (see [Images](usage.md#images)), so it fits within 2,000 by 2,000 pixels and keeps its aspect ratio. It refuses anything that is not a regular file, such as a pipe or device
- `write`: create or replace a file. It refuses anything that is not a regular file, such as a pipe or device
- `edit`: replace text in a file. The text must match exactly once, unless `replace_all` is true, which replaces every match and reports how many replacements it made. The count shows on a line such as `edit: 3 replacements`, in the interface, in resumed sessions and in print mode. In a file with Windows line endings (CRLF), it also matches text written with plain line endings, and writes the new text with CRLF line endings. A file counts as using CRLF when its first line ends with CRLF. It refuses files larger than 10 MiB and anything that is not a regular file, such as a pipe or device. If the file changes while it is being edited, for example by a subagent editing it at the same time, the edit fails instead of undoing the other change
- `agent`: run tasks in subagents. See [Subagents](#subagents)
- `web_search`: search the web. See [Web search](#web-search)
- `web_fetch`: read a web page or PDF. See [Web fetch](#web-fetch)

At start, the interface lists these tools on one line, before the MCP servers, such as `loaded tools: agent, bash, edit, read, web_fetch, web_search, write`.

The agent can also use tools from [MCP servers](mcp.md).

`write` and `edit` write a temporary file in the same folder and then replace the file with it, so a failed write, for example on a full disk, keeps the old file. The file keeps its permissions and group, so a script stays executable, and a symlink stays a symlink, with the file it points to replaced. A file with other hard links, or owned by someone else, is written in place instead, so every link sees the new text and the owner does not change.

`bash` returns at most 20,000 bytes of output. Standard output and standard error each get half, and a stream that needs less leaves the rest to the other. A longer stream keeps its start and its end, with a note of how many bytes were left out in between. It returns once the command exits, even if a background process it started keeps running. Output such a process writes after the command exits is kept while it keeps coming, until it pauses for a tenth of a second or a second has passed. If a command times out, `bash` returns the output so far, followed by the timeout notice. `bash` and `!` commands and MCP servers run without the terminal, so programs that prompt on it, such as `sudo`, `ssh` or `git` asking for a password, fail at once instead of waiting for input.

Consecutive `read` calls show as one line with the paths separated by commas, for example `read src/main.rs (2), README.md`. A number in brackets shows how many times a file was read. A read with `offset` or `limit` shows its line range, for example `src/tools.rs:325-354`, or `src/tools.rs:325-` when only `offset` is given. A failed read starts a new line. In print mode, the line is printed when the next tool, text or notice arrives.

## Web search

`web_search` is Anthropic's [web search tool](https://docs.claude.com/en/docs/agents-and-tools/tool-use/web-search-tool) (`web_search_20250305`). It runs on Anthropic's servers and works with a Pro or Max sign-in. The model decides when to search, and there is no limit on searches. Subagents can use it too.

Each search shows its query, such as `web_search latest stable Rust release`, followed by the number of results, such as `web_search: 9 results`, or why it failed, such as `web_search failed: too_many_requests`. When a reply cites pages, a `Sources:` list with each page's title and URL follows the reply. This works in the interface, in resumed sessions and in print mode, where the search lines go to standard error and the sources to standard output with the reply.

## Web fetch

`web_fetch` is Anthropic's [web fetch tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-fetch-tool) (`web_fetch_20250910`). It runs on Anthropic's servers and works with a Pro or Max sign-in. It reads web pages as text and PDF files as documents, but not pages that need JavaScript to show their content. It can fetch only URLs that appear earlier in the conversation, for example in your prompt or in search results. Because it runs on Anthropic's servers, it cannot reach `localhost` or private networks; the agent can use `curl` through `bash` for those. The model decides when to fetch, and there is no limit on fetches or page size. Subagents can use it too.

Each fetch shows its URL, such as `web_fetch https://example.com`, followed by the page title, or the URL when the page has none, such as `web_fetch: Example Domain`, or why it failed, such as `web_fetch failed: url_not_accessible`. Citations are turned on, so pages a reply cites are listed under `Sources:` as for web search, in the interface, in resumed sessions and in print mode.

## Paused replies

The API may pause a long reply partway through. rust-coding-agent then sends the request again so the reply can carry on.

## Subagents

The `agent` tool takes a list of tasks, each with a short `description` and a `prompt`, and a `mode`:

- `parallel` runs the tasks at the same time
- `sequential` runs them one after another, and adds each task's final reply to the next task's prompt. If a task fails, the tasks after it do not run

Each task runs in its own subagent with a new conversation. A subagent uses the same model, effort level, system prompt, instructions, skills and tools as the main agent, except `agent`, so it cannot start subagents of its own. Its conversation is not saved. The main agent gets one text block per task, with that task's final reply, or why it failed or did not run. If a task finishes without any reply text, its block says so instead of being empty.

The input, output and cache tokens that subagents use count towards the totals on the status line, which update while the subagents work and stay the same in resumed sessions. Context use and output speed come from the main agent only. If you cancel a prompt while subagents work, the tokens of the subagent replies that had finished still count. The tokens of a subagent that fails still count too.

In the interface, the `agent` call shows its mode and number of tasks, such as `agent parallel, 2 tasks`. Below it, each task shows its number and description, such as `[agent 1] list files`. Once the subagent calls a tool, the number of tool calls so far follows, such as `[agent 1] list files · 3 tool calls`. After its first reply, the subagent's own context use and stats follow, in the same form as on the status line, such as `[agent 1] list files · 3 tool calls · 4%/1m · ↑312 ↓71k R7m W472k 107t/s`. They update while it works and stay after it finishes. Nothing else a subagent writes or does is shown, not even its final reply. If a task fails or does not run, the first line of why shows below it. A resumed session shows the tasks the same way, but without the number of tool calls or stats, because subagent conversations are not saved.

In print mode, the `agent` call shows its mode and number of tasks, such as `agent sequential, 2 tasks`. Below it, each task shows one line when it finishes, as in the interface: its number, description, number of tool calls and stats, such as `[agent 1] say hi · 3 tool calls · 12%/200k · ↑4 ↓338 R6k W88 42t/s`. If a task fails or does not run, the first line of why follows on the next line.
