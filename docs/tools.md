# Tools

The agent can use these tools:

- `bash`: run a shell command. An optional `timeout` sets the seconds before the command is stopped, with all its background processes. Without it there is no limit. With `run_in_background`, it returns at once. See [Background commands](#background-commands)
- `read`: read a file. It reads only up to the requested lines and stops once it has 20,000 bytes of output, so the first lines of a huge file come back straight away. It refuses anything that is not a regular file, such as a pipe or device
- `write`: create or replace a file. It refuses anything that is not a regular file, such as a pipe or device
- `edit`: replace text in a file. The text must match exactly once, unless `replace_all` is true, which replaces every match and reports how many replacements it made. The count shows on a line such as `edit: 3 replacements`, in the interface, in resumed sessions and in print mode. In a file with Windows line endings (CRLF), it also matches text written with plain line endings, and writes the new text with CRLF line endings. A file counts as using CRLF when its first line ends with CRLF. It refuses files larger than 10 MiB and anything that is not a regular file, such as a pipe or device. If the file changes while it is being edited, for example by a subagent editing it at the same time, the edit fails instead of undoing the other change

- `agent`: run tasks in subagents. See [Subagents](#subagents)
- `bash_stop`: stop a background command. See [Background commands](#background-commands)
- `web_search`: search the web. See [Web search](#web-search)

At start, the interface lists these tools on one line, after the MCP servers, such as `loaded tools: agent, bash, bash_stop, edit, read, web_search, write`.

The agent can also use tools from [MCP servers](mcp.md).

`write` and `edit` write a temporary file in the same folder and then replace the file with it, so a failed write, for example on a full disk, keeps the old file. The file keeps its permissions and group, so a script stays executable, and a symlink stays a symlink, with the file it points to replaced. A file with other hard links, or owned by someone else, is written in place instead, so every link sees the new text and the owner does not change.

`bash` returns at most 20,000 bytes of output. Standard output and standard error each get half, and a stream that needs less leaves the rest to the other. A longer stream keeps its start and its end, with a note of how many bytes were left out in between. It returns once the command exits, even if a background process it started keeps running. If a command times out, `bash` returns the output so far, followed by the timeout notice. `bash` and `!` commands and MCP servers run without the terminal, so programs that prompt on it, such as `sudo`, `ssh` or `git` asking for a password, fail at once instead of waiting for input.

Consecutive `read` calls show as one line with the paths separated by commas, for example `read src/main.rs (2), README.md`. A number in brackets shows how many times a file was read. A read with `offset` or `limit` shows its line range, for example `src/tools.rs:325-354`, or `src/tools.rs:325-` when only `offset` is given. A failed read starts a new line. In print mode, the line is printed when the next tool, text or notice arrives.

## Background commands

With `"run_in_background": true`, `bash` starts the command and returns at once with its task number, such as `started background task 1`. The agent can carry on, and you can send prompts, while the command runs. This suits long waits such as `gh run watch --exit-status` for a CI run. The call shows as `bash in the background`, with the command below it.

When the command finishes, its output comes back in a message to the model, with the same limits as other `bash` output. It shows as `background task 1 finished`, with the output below it. Until then, the status line shows how many background commands are running, such as `1 background`. If the agent is idle, a turn starts at once with that message. If the agent is working, the message waits until the current turn ends. `timeout` works as for other commands; output after a timeout comes back the same way.

`bash_stop` with the task number stops a background command and all its processes, and no message comes back for it. Esc cancels the current turn but leaves background commands running. Quitting, or a stop signal such as Ctrl+C, stops them all. In print mode, rust-coding-agent waits for every background command to finish, and answers each one, before it exits. Resumed sessions show the finished messages, but commands that were running when the session ended do not come back.

Subagents cannot run commands in the background or use `bash_stop`. They do not get `run_in_background` or `bash_stop` at all.

## Web search

`web_search` is Anthropic's [web search tool](https://docs.claude.com/en/docs/agents-and-tools/tool-use/web-search-tool) (`web_search_20250305`). It runs on Anthropic's servers and works with a Pro or Max sign-in. The model decides when to search, and there is no limit on searches. Subagents can use it too.

Each search shows its query, such as `web_search latest stable Rust release`, followed by the number of results, such as `web_search: 9 results`, or why it failed, such as `web_search failed: too_many_requests`. When a reply cites pages, a `Sources:` list with each page's title and URL follows the reply. This works in the interface, in resumed sessions and in print mode, where the search lines go to standard error and the sources to standard output with the reply.

## Paused replies

The API may pause a long reply partway through. rust-coding-agent then sends the request again so the reply can carry on.

## Subagents

The `agent` tool takes a list of tasks, each with a short `description` and a `prompt`, and a `mode`:

- `parallel` runs the tasks at the same time
- `sequential` runs them one after another, and adds each task's final reply to the next task's prompt. If a task fails, the tasks after it do not run

Each task runs in its own subagent with a new conversation. A subagent uses the same model, effort level, system prompt, instructions, skills and tools as the main agent, except `agent`, so it cannot start subagents of its own. Its conversation is not saved. The main agent gets one text block per task, with that task's final reply, or why it failed or did not run.

The cache tokens that subagents use count towards the totals on the status line, which update while the subagents work and stay the same in resumed sessions. Context use and output speed come from the main agent only. If you cancel a prompt while subagents work, the tokens of the subagent replies that had finished still count. The tokens of a subagent that fails still count too.

In the interface, the `agent` call shows its mode and number of tasks, such as `agent parallel, 2 tasks`. Below it, each task shows its number and description, such as `[agent 1] list files`. Once the subagent calls a tool, the number of tool calls so far follows, such as `[agent 1] list files · 3 tool calls`. After its first reply, the subagent's own context use and stats follow, in the same form as on the status line, such as `[agent 1] list files · 3 tool calls · 4%/1m · R7m W472k 107t/s`. They update while it works and stay after it finishes. Nothing else a subagent writes or does is shown, not even its final reply. If a task fails or does not run, the first line of why shows below it. A resumed session shows the tasks the same way, but without the number of tool calls or stats, because subagent conversations are not saved.

In print mode, the `agent` call shows its mode and number of tasks, such as `agent sequential, 2 tasks`. Below it, each task shows one line when it finishes, as in the interface: its number, description, number of tool calls and stats, such as `[agent 1] say hi · 3 tool calls · 12%/200k · R6k W88 42t/s`. If a task fails or does not run, the first line of why follows on the next line.
