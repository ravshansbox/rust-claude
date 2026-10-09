# rust-claude

A small coding agent for the terminal, written in Rust. It talks to Claude and can run shell commands and read, write and edit files in the current directory.

## Features

Core:

- Sign in with a Claude Pro or Max account, with automatic token renewal
- Streamed replies
- Built-in tools: `bash`, `read`, `write` and `edit`
- Interactive terminal interface

Command line:

- Print mode (`-p` / `--print`) to run one prompt and print the answer
- `--image <path>` to send images in print mode
- `-c` / `--continue` to continue the latest session in the current folder
- Syntax-highlighted tool calls on standard error in print mode
- `--config-dir <path>` to keep sign-in, settings, sessions and other files in another folder
- Help with `-h` / `--help`

Model and thinking:

- `/model` picker with the models available to your account
- `/model <id>`, which checks the id before switching
- Known context windows, output limits and thinking modes for each model
- Thinking levels with `/thinking`, `/thinking <level>` and Shift+Tab
- Thinking text shown in the conversation
- Model and thinking level saved to `~/.rust-claude/settings.json`
- `--model` and `--thinking` options for one run

Sessions and context:

- Sessions saved as JSON Lines files
- `/resume` picker with previews and how long ago each session changed
- Resumed sessions show messages, tool calls and failures as they appeared live
- `/new` to start a new session
- `/compact` to summarise the conversation
- `/context` to show what fills the context
- Automatic compaction when the context is 80% full
- Finished tool calls kept when a prompt is cancelled or fails
- Prompt caching

Reliability:

- Retries for rate limits, overload, server errors and connection errors, with back-off and `retry-after`
- Requests that get no data for 5 minutes, or cannot connect within 30 seconds, fail and are retried instead of hanging
- Timed-out commands stopped with all their background processes
- Optional `timeout` for `bash`
- Command output capped at 20,000 bytes as it arrives

Tools:

- `edit` with `replace_all` and a replacement count
- `edit` support for files with Windows line endings
- `read` with `offset` and `limit`, cutting long output at whole lines
- `write` creates missing parent folders
- Diff preview for `edit` and first-lines preview for `write`
- Syntax highlighting in those previews
- Consecutive `read` calls grouped on one line, with counts and line ranges

Status line:

- Input, output, cache read and cache write tokens
- Cache hit rate
- Output speed in tokens per second
- Context use
- 5-hour and 7-day quota with reset times, loaded at start and updated after each reply
- Folder and Git branch

Input and editing:

- Multi-line input with Shift+Enter or Alt+Enter
- Prompts queued while the agent works
- `!command` to run a shell command and share its output with the model
- Prompt history with Up and Down
- Ctrl+R to search prompts from this folder or from all folders
- Up and Down move between wrapped input rows
- Move and delete by word
- Ctrl+A and Ctrl+E to jump to the start or end
- Bracketed paste
- Tab completion for commands
- `@` file picker

Display:

- Markdown replies, with tables wrapped to the window width
- Light and dark theme detection
- Mouse wheel scrolling
- The view stays still while you read earlier text and a reply streams
- Spinner with status text

Images:

- Paste images with Ctrl+V on macOS, Wayland and X11
- Image markers in the prompt; deleting a marker removes the image
- Large or unsupported images resized and converted
- Session images saved to disk

Instructions and skills:

- `AGENTS.md` from your home folder and the current directory
- [Agent Skills](https://agentskills.io/specification) from project and global folders, with ignore files, validation and collision warnings
- `/skill:name` commands, marked `[global]` or `[project]`
- Skills listed in the system prompt, unless `disable-model-invocation` is set

MCP:

- [MCP servers](#mcp-servers) over stdio, configured globally or per project
- MCP servers started in the background in the interface
- `~/` expansion, `env`, `cwd`, `timeout` and `enabled` options
- Paginated tool lists, `ping` replies, and server error output shown in errors
- Clear errors for HTTP and SSE servers, which are not supported yet

Other:

- [Architecture decision records](#decisions)

## Install

You need [Rust](https://rustup.rs).

```sh
cargo install --locked --force --git https://github.com/ravshansbox/rust-claude
```

This puts `rust-claude` in `~/.cargo/bin`. `--force` makes cargo rebuild even if this version is already installed, so the same command also upgrades or reinstalls.

**Upgrade or reinstall:** run the `cargo install` command again.

**Uninstall:**

```sh
cargo uninstall rust-claude
rm -r ~/.rust-claude                     # optional: sign-in, settings, sessions, prompt history, skills and MCP config
```

## Sign in

On first run, rust-claude prints a sign-in link for your Claude Pro or Max account to standard error and asks for the code. It saves the sign-in to `~/.rust-claude/auth.json` and renews it when it expires. It writes a temporary file and then replaces `auth.json` with it, so other running copies never read a half-written file and a crash keeps the previous sign-in. Before renewing, it checks `auth.json` for a sign-in another running rust-claude has already renewed, and uses that one, so several running copies stay signed in. If renewing fails, it checks `auth.json` again before asking you to sign in.

## Usage

Start the interactive interface:

```sh
rust-claude
```

Run one prompt and print the answer with `-p` or `--print`. The argument after it is always the prompt, even when it starts with `-`:

```sh
rust-claude -p "explain src/main.rs"
```

Send images with the prompt with `--image <path>`. Repeat it for more images:

```sh
rust-claude -p "what is wrong in this screenshot?" --image error.png
```

In print mode, tool calls go to standard error, with syntax highlighting when standard error is a terminal.

Press Ctrl+C in print mode to stop. rust-claude stops running commands and MCP servers, saves the session with the prompt marked as cancelled, prints `cancelled` to standard error and exits with status 130.

Choose the model and thinking level for one run with `--model <id>` and `--thinking <level>`. See [Settings](#settings).

Continue the latest session started in the current folder with `-c` or `--continue`. It works in the interface and in print mode:

```sh
rust-claude -c
rust-claude -c -p "now add tests"
```

If no session in the folder can be continued, rust-claude stops with an error.

Keep sign-in, settings, sessions, prompt history, skills and MCP config in another folder with `--config-dir <path>`. It works in the interface and in print mode. Wherever this README mentions `~/.rust-claude`, it means this folder when the option is given. `~/AGENTS.md` and `~/.agents/skills/` still come from your home folder.

```sh
rust-claude --config-dir ~/work/.rust-claude
```

Sessions hold whole conversations and tool output, so rust-claude creates `~/.rust-claude` and the folders and files it writes there so that only you can read them. Folders and files made by older versions keep their permissions.

Show help with `-h` or `--help`.

Markdown tables in replies wrap their cells to fit the window width.

Edit tool calls show a diff, and write tool calls show the first 10 lines of the new file. Both use syntax highlighting based on the file extension. Removed lines have a red background and added lines have a green background. Leading spaces are removed from each line to save room.

## Commands

| Command | Action |
| --- | --- |
| `/new` | Start a new session |
| `/resume` | Resume a previous session |
| `/compact` | Summarise the conversation to free context |
| `/context` | Show what fills the context |
| `/model [id]` | Select a model. An id must be in the list of available models |
| `/thinking [level]` | Select a thinking level |
| `/skill:name [request]` | Run a skill, with an optional request |
| `/quit` | Quit |

Typing `/` lists the commands and skill commands that start with the input. The list shows up to 10 at a time, and a longer list scrolls as you move through it, with a scrollbar on the right.

The `/resume`, `/model` and `/thinking` pickers open in a box above the input. The box shows up to 10 items, and the conversation stays visible above it. A longer list scrolls as you move through it, with a scrollbar on the right.

## Files

Type `@` to pick a file. The list shows up to 10 project files whose path contains the text after `@`, ignoring case. Use Up / Down to select, and Tab or Enter to insert `@path` into the prompt. Esc closes the list. In a Git repository the list holds tracked files and untracked files that `.gitignore` does not exclude. Otherwise it holds all files, except hidden ones and those in `target`.

## Prompt history

Press Ctrl+R to search earlier prompts. The search opens in a box above the input, with the conversation still visible above it. The box shows up to 10 prompts, with a scrollbar when there are more. Type to filter the list, ignoring case. The list has two tabs: **Folder** holds prompts sent from the current folder, and **All** holds prompts from every folder, with the folder name next to each one. Left and Right switch tabs, Up / Down select, Enter puts the prompt in the input to edit, and Esc or Ctrl+R closes the list. Repeated prompts show once, at their newest position.

In the interface, rust-claude saves each prompt you send, including `!` commands, skill commands and queued prompts, to `~/.rust-claude/history.jsonl` with the folder it was sent from. The **All** tab reads this file. At start-up, rust-claude loads the prompts sent from the current folder, so Up / Down and the **Folder** tab keep them after a restart or `/new`. Resuming a session, with `/resume` or `--continue`, also adds its prompts to Up / Down and the **Folder** tab for that run. Prompts sent in print mode or before this file existed are not in it.

## Keys

| Key | Action |
| --- | --- |
| Enter | Send prompt, or queue it while the agent works |
| Shift+Enter / Alt+Enter | Add a new line |
| Esc | Cancel the running prompt or model lookup, or quit when idle |
| Ctrl+C | Clear input, or quit when input is empty |
| Ctrl+D | Quit when input is empty |
| Ctrl+V | Paste an image from the clipboard |
| Ctrl+R | Search prompt history |
| Shift+Tab | Cycle thinking level |
| Up / Down | Browse prompt history, move between input rows, or scroll |
| Page Up / Page Down | Scroll by a page |
| Home / End | Scroll to top or bottom |
| Left / Right | Move cursor by one character, keeping emoji such as 👍🏽 whole |
| Alt+Left / Alt+B, Alt+Right / Alt+F | Move by word |
| Ctrl+A / Ctrl+E | Jump to start or end of input |
| Alt+Backspace / Ctrl+W | Delete previous word |
| Tab | Complete a command or file |

While a reply streams, the view follows new text only when it is scrolled to the bottom. If you scroll up, the view stays where it is.

Shift+Enter works in terminals that support the kitty keyboard protocol, such as kitty, WezTerm, Ghostty and iTerm2. Other terminals send it as Enter, so use Alt+Enter there.

## Shell commands

Start a prompt with `!` to run the rest as a shell command, for example `!git status`. The command runs in the current directory, and its output shows in the conversation. It is also added to the conversation, so the model sees the command and its output with your next prompt. It is not sent on its own.

Output is capped at 20,000 bytes, as with the `bash` tool. The command has no time limit; press Esc to stop it. Resumed sessions show these commands and their output.

## Queued prompts

While the agent works, Enter queues the prompt instead of sending it. Queued prompts show below the spinner. After the next round of tool calls, rust-claude adds them to the conversation, so the model reads them before it carries on. If the reply ends first, they go out together as a new prompt. Commands such as `/new`, skill commands and `!` commands are not queued.

If you cancel with Esc, or the prompt fails, the queued prompts go back into the input so you can edit or resend them.

## Status line

The status line is a single line below the input. It shows the folder and Git branch, the model and thinking level, context use, and token use for the session: input (↑), output (↓), cache reads (R), cache writes (W), the cache hit rate (CH) and the average output speed in tokens per second (tps). A `·` separates each part. It wraps when the terminal is too narrow. The folder and branch update after each tool call and `!` command.

It also shows how much of the 5-hour and 7-day quota is left and when each resets. rust-claude loads it in the background at start, so you can send a prompt straight away, and updates it after each reply. When only one of them is known, it is labelled, for example `5h 95% 2h16m` or `7d 81% 2d12h`.

## Settings

| Setting | Option | `~/.rust-claude/settings.json` key | Default |
| --- | --- | --- | --- |
| Model | `--model <id>` | `model` | `claude-opus-5-5` |
| Thinking level | `--thinking <level>` | `thinking_level` | `medium` |

Thinking levels: `low`, `medium`, `high`, `xhigh`, `max`.

Most models use adaptive thinking, with the level sent as the effort. Opus 4.6 and Sonnet 4.6 do not support `xhigh`, so they get `high` instead. Haiku 4.5, Sonnet 4.5 and Opus 4.5 do not support adaptive thinking, so the level sets a thinking budget instead: 4,000 tokens for `low`, 16,000 for `medium`, 32,000 for `high`, and one token less than the output limit for `xhigh` and `max`. Opus 4.5 also gets the effort for `low`, `medium` and `high`. Models that rust-claude doesn't know use adaptive thinking.

Each reply may use up to the model's full output limit (64,000 or 128,000 tokens). Models that rust-claude doesn't know get 8,192 tokens.

Options take priority over the settings file and apply to that run only. They work in the interface and in print mode. rust-claude does not check the `--model` id, so an unknown id fails on the first request.

An unknown thinking level prints a warning to standard error and uses `medium`.

Changing the model or thinking level in the interface saves only the setting you changed.

## Instructions

rust-claude adds `AGENTS.md` from your home folder and from the current directory to the system prompt.

The system prompt tells the model to put questions to the user in bold and give each one lettered options, with the recommended option marked. Questions are numbered when there is more than one.

The system prompt also tells the model to search code with `ast-grep`, and to fall back to `ripgrep` for plain text, comments, strings and files `ast-grep` cannot parse. Both run through the `bash` tool. At start, rust-claude looks for `ast-grep` and `rg` on `PATH`. For each one it cannot find, it shows a warning such as `rg not found on PATH`, in the interface or on standard error in print mode, and leaves that program out of the system prompt.
It also tells the model to prefer `edit` and `write` over `bash` for changing files.

## Skills

rust-claude supports [Agent Skills](https://agentskills.io/specification). A skill is a folder with a `SKILL.md` file that starts with YAML frontmatter:

```markdown
---
name: pdf-tools
description: Extract text and tables from PDF files. Use when reading, converting, or inspecting PDFs.
---

# PDF tools

Instructions for the model.
```

rust-claude looks for skills in these places, in this order:

1. `.rust-claude/skills/` in the current directory
2. `.agents/skills/` in the current directory and each parent directory, up to the Git repository root, or up to the filesystem root outside a repository
3. `~/.rust-claude/skills/`
4. `~/.agents/skills/`

A folder that holds `SKILL.md` is a skill, and rust-claude does not look inside it for more skills. Other folders are searched for `SKILL.md` files. Markdown files with a `description` also load as skills when they sit directly in a `.rust-claude/skills/` folder, or below the top level of an `.agents/skills/` folder. Hidden folders, `node_modules` and paths listed in `.gitignore`, `.ignore` or `.fdignore` are skipped. As in Git, the patterns in an ignore file apply relative to the folder that holds it. Symlinked folders are followed, but each folder is searched only once, so a symlink loop does not stall the search.

The system prompt lists each skill's name, description and file path. The model reads the full file with `read` when a task matches the description. Set `disable-model-invocation: true` in the frontmatter to leave a skill out of the system prompt.

Type `/skill:name` to run a skill yourself. It sends the skill's instructions to the model, followed by any text after the name. That text can start on the same line or on the next one. The conversation shows `[skill] name` in place of the instructions. Tab completes skill commands and marks each one as `[global]` or `[project]`.

The skill name comes from `name` in the frontmatter, or from the folder name. Names should use lowercase letters, numbers and single hyphens, up to 64 characters. Descriptions can have up to 1,024 characters. A skill without a description does not load. If two skills share a name, the first one found wins. At start, the interface lists the loaded global skills, then the loaded project skills, and shows a warning for each problem. Global skills come from your home folder (places 3 and 4). Project skills come from the current directory and its parents (places 1 and 2).

## Retries

If a request fails with a rate limit (429), an overloaded API (529), a server error (5xx) or a connection error before the reply starts, rust-claude tries again up to 3 times. It waits 1, 2 and then 4 seconds, or as long as the `retry-after` header asks. It does not retry when `retry-after` is longer than 60 seconds. A notice shows each retry. Tokens that a failed attempt already used still count toward the session's token use.

A request counts as a connection error when it cannot connect within 30 seconds, or when no data arrives for 5 minutes. If no data arrives after the reply has started, the prompt fails with an error instead of hanging.

## Images

Press Ctrl+V to paste an image from the clipboard. It adds a marker such as `[image 1]` at the cursor. Only images whose marker is still in the prompt are sent, so deleting the marker removes the image. On macOS, rust-claude reads the clipboard with `osascript`. On Linux, it uses `wl-paste` under Wayland and `xclip` otherwise.

rust-claude sends PNG, JPEG, GIF and WebP images as they are when they fit the limits. It scales larger images down to at most 2,000 pixels on the long edge, and converts other formats to PNG. Photos stored sideways with an Exif orientation, as phone cameras often save them, are turned upright and re-encoded. If an image is still larger than 5 MB in base64, it uses JPEG at lower quality, then halves the size until it fits.

## Sessions

Sessions are saved to `~/.rust-claude/sessions/` as JSON Lines files. The first line records the folder the session started in, which `--continue` uses. Sessions saved before this line was added are not found by `--continue`, but `/resume` still lists them. Images sent with a prompt are saved in a folder named after the session, such as `~/.rust-claude/sessions/<id>/`, and the session file refers to them by name.

While a prompt runs, the session is saved after each finished round of tool calls, so a crash or a closed terminal loses at most the round in progress. If a crash or a full disk cuts a save short, resuming the session keeps the messages before the cut, leaves out tool calls whose results were cut off, and removes the rest from the file.

A session is open in one rust-claude at a time, so two copies never write to the same file. Resuming a session that another running rust-claude has open, with `/resume` or `--continue`, shows an error instead.

Quitting while a prompt runs cancels it and saves the session first. Quitting while `/model` loads the model list stops that request. Requests still waiting when you quit, such as a prompt sent just before, are not started.

A cancelled or failed prompt stays in the session with its finished tool calls, so the model sees them in the next request.

Newer models only use thinking from earlier replies while the system prompt, tools and messages sent before it stay the same. These can change during a session, for example when an MCP server connects after the first prompt, or when a resumed session finds different `AGENTS.md` files or skills. When the API rejects a request for this reason, rust-claude shows a notice, sends the request once more asking the API to drop the thinking that no longer matches, and keeps asking for the rest of the session. The session file records this, so it also applies after resuming.

`/compact` asks the model to summarise the conversation. Later requests send the summary in place of the earlier messages. The session file keeps the full conversation and a compaction entry that holds the summary. A resumed session shows the earlier messages, then a `compacted conversation` notice. Esc cancels a running compaction.

rust-claude also compacts automatically when the context is 80% full: before sending a new prompt, and after each round of tool calls. A prompt sent at that point, or queued during that round, is kept word for word after the summary. Models that rust-claude doesn't know have no known context window, so they only compact with `/compact`.

`/context` shows how many tokens each part of the context uses: the system prompt, instructions, skills, built-in tools, MCP tools and messages, and how much of the context window is free. Each part is estimated from its length, then scaled so that the parts add up to the context use on the status line. That figure comes from the token count of the last reply, plus an estimate for anything added since. Images are not counted.

`/resume` reads each session only up to its first prompt to build the list. A session that is damaged before its first prompt is left out of the list. Resuming a session that is damaged later in the file keeps the messages before the damage, as after a crash. A resumed session shows messages, tool calls and failed tool calls as they appeared live.

## Tools

The agent can use these tools:

- `bash`: run a shell command
- `read`: read a file
- `write`: create or replace a file
- `edit`: replace text in a file. The text must match exactly once, unless `replace_all` is true, which replaces every match and reports how many replacements it made. The count shows on a line such as `edit: 3 replacements`, in the interface, in resumed sessions and in print mode. In a file with Windows line endings (CRLF), it also matches text written with plain line endings, and writes the new text with CRLF line endings. A file counts as using CRLF when its first line ends with CRLF

The agent can also use tools from [MCP servers](#mcp-servers).

`bash` keeps only the first 20,000 bytes of output and discards the rest as it arrives. It returns once the command exits, even if a background process it started keeps running. If a command times out, `bash` returns the output so far, followed by the timeout notice.

Consecutive `read` calls show as one line with the paths separated by commas, for example `read src/main.rs (2), README.md`. A number in brackets shows how many times a file was read. A read with `offset` or `limit` shows its line range, for example `src/tools.rs:325-354`, or `src/tools.rs:325-` when only `offset` is given. A failed read starts a new line. In print mode, the line is printed when the next tool, text or notice arrives.

## MCP servers

rust-claude connects to [Model Context Protocol](https://modelcontextprotocol.io) servers over stdio and gives their tools to the model. Add servers to `~/.rust-claude/mcp.json`, or to `.rust-claude/mcp.json` in a project. The format uses the standard `mcpServers` shape:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]
    }
  }
}
```

- `command` is a single executable and `args` its arguments. `env` sets environment variables and `cwd` the working folder. A leading `~/` in `command`, an argument or `cwd` names the home folder.
- `timeout` sets the time limit for each request in seconds (default 60).
- `enabled: false` keeps an entry without connecting to it.
- `type` is optional. When present, it must be `stdio`. HTTP and SSE servers are not supported yet.
- Server names may only contain letters, digits, `_` and `-`.

Project entries replace global entries with the same name. rust-claude starts project servers without asking (see [ADR 3](docs/adr/0003-start-project-mcp-servers-without-approval.md)).

In the interface, rust-claude starts all servers in the background, so you can type and send prompts straight away. Each server's tools become available once it is ready, with a notice such as `loaded project MCP server: docs (3 tools)`. A prompt sent before then goes without those tools, and a server that becomes ready while a prompt runs joins after that prompt. The interface also shows a notice for each invalid entry or server that failed to connect. In print mode, rust-claude waits for all servers before sending the prompt, and the notices for invalid entries and failed servers go to standard error. Tools are named `mcp__<server>__<tool>`, with other characters replaced by `_` and cut to 64 characters. `anyOf`, `oneOf` and `allOf` at the top level of a tool's input schema are dropped, because the API rejects them. Text results longer than 20,000 bytes are cut. Images, audio and binary resources show as short placeholders. Servers stop when rust-claude quits.

## Decisions

Architecture decision records are in [docs/adr](docs/adr):

- [1. Identify as Claude Code](docs/adr/0001-identify-as-claude-code.md)
- [2. Run tools without approval](docs/adr/0002-run-tools-without-approval.md)
- [3. Start project MCP servers without approval](docs/adr/0003-start-project-mcp-servers-without-approval.md)

## Development

Before each commit, run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`. Tests keep sign-in, sessions and other files in a temporary folder, not in `~/.rust-claude`.

## Licence

MIT. See [LICENSE](LICENSE).
