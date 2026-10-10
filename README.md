# rust-claude

A small coding agent for the terminal, written in Rust. It talks to Claude and can run shell commands and read, write and edit files in the current directory.

## Features

Core:

- Sign in with a Claude Pro or Max account, with automatic token renewal
- Streamed replies
- Built-in tools: `bash`, `python`, `read`, `write` and `edit`
- Interactive terminal interface

Command line:

- Print mode (`-p` / `--print`) to run one prompt and print the answer
- `--image <path>` to send images in print mode
- `-c` / `--continue` to continue the latest session in the current folder
- Syntax-highlighted tool calls on standard error in print mode
- `--config-dir <path>` to keep sign-in, settings, sessions and other files in another folder
- Help with `-h` / `--help`

Model, effort and thinking:

- `/model` picker with the latest model in each class available to your account: Fable, Opus, Sonnet and Haiku
- `/model <id>`, which checks the id before switching
- Ctrl+P to switch between the latest models: Fable, Opus, Sonnet, Haiku
- Known context windows and output limits for the latest model in each class
- Effort levels with `/effort`, `/effort <level>` and Shift+Tab, and `off` to turn thinking off on Sonnet and Haiku
- Thinking text shown in the conversation
- Model and effort level saved to `~/.rust-claude/settings.json`
- `--model` and `--effort` options for one run

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
- Optional `timeout` for `bash` and `python`
- Commands run without the terminal, so password prompts fail instead of hanging
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
- Bracketed paste; tabs in pasted text show as four spaces in the input and the conversation
- Tab completion for commands
- `@` file picker

Display:

- Markdown replies, with tables wrapped to the window width
- Tabs in replies, tool output and `!` command output shown as four spaces
- Light and dark theme detection
- Mouse wheel scrolling
- Drag over the conversation to select text. Releasing the button copies it to the clipboard with an OSC 52 escape sequence, so the terminal must allow OSC 52. Each screen row is copied as its own line. A "Copied" box shows at the top right for 2 seconds
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
- `/skill:name` commands, marked `[global]` or `[project]`; run from the home folder, `~/.rust-claude` counts as global only
- Skills listed in the system prompt, unless `disable-model-invocation` is set

MCP:

- [MCP servers](#mcp-servers) over stdio or Streamable HTTP, configured globally or per project
- MCP servers started in the background in the interface, each shown loading until ready
- `~/` expansion, `env`, `cwd`, `timeout` and `enabled` options
- Paginated tool lists, `ping` replies, and server error output shown in errors
- A server that sends the same page of its tool list twice fails with an error instead of loading forever
- A server that writes a single line longer than 32 MiB is disconnected with an error, so it cannot use up memory
- Streamable HTTP servers with `headers`, sessions and retries; a clear error for the older SSE transport
- OAuth sign-in to HTTP servers with `/mcp login <server>` and `/mcp logout <server>`, which complete server names

Other:

- [Architecture decision records](#decisions)

## Install

You need [Rust](https://rustup.rs).

```sh
curl -fsSL https://raw.githubusercontent.com/ravshansbox/rust-claude/main/install.sh | sh
```

[install.sh](install.sh) downloads the source of the latest [release](https://github.com/ravshansbox/rust-claude/releases) into a temporary folder, builds it with `cargo install --locked --force` in `~/.rust-claude/build`, the same folder that updates use, and puts `rust-claude` in `~/.cargo/bin`. It stops with an error if `cargo` is not on `PATH`. `--force` makes cargo rebuild even if this version is already installed, so the same command also upgrades or reinstalls.

**Upgrade or reinstall:** run the same command again.

**Updates:** when the interface starts, rust-claude checks the [latest release](https://github.com/ravshansbox/rust-claude/releases/latest) in the background, every time. If it is newer, rust-claude shows `update v0.2.0 available`, then `downloading v0.2.0` and `building v0.2.0`, and builds and installs it with `cargo install` while you keep working. When it is done, it shows `installed v0.2.0, restart rust-claude to use it`. If `cargo` is not on `PATH`, it shows a warning instead. A failed build shows the first error from cargo. Builds go in `~/.rust-claude/build` (or `build` in the `--config-dir` folder), so later updates reuse the dependencies that did not change and build faster. When `rustc -vV` reports a different compiler, rust-claude empties that folder first, because cargo cannot reuse the old files. To free the space, delete the folder. A failed check, for example when offline, shows nothing. Debug builds, such as from `cargo run`, and builds from `main` (`v0.0.0`) do not check. To turn the check off, set `check_for_updates` to `false` in `settings.json`.

**Uninstall:**

```sh
cargo uninstall rust-claude
rm -r ~/.rust-claude                     # optional: sign-in, settings, sessions, prompt history, skills and MCP config
```

## Sign in

On first run, rust-claude prints a sign-in link for your Claude Pro or Max account to standard error and asks for the code. Paste the whole code the page shows, including the part after `#`; rust-claude checks that part against the sign-in link and stops with an error if it is missing or does not match. It saves the sign-in to `~/.rust-claude/auth.json` and renews it when it expires. Cancelling a prompt with Esc while the sign-in renews still saves the renewed sign-in. Quitting, or stopping print mode with Ctrl+C, waits up to 5 seconds for a renewal in progress to be saved. If it cannot save a new or renewed sign-in, it keeps using it until it quits. It shows the error on standard error after you sign in, and in the renewal notice after it renews. It writes a temporary file and then replaces `auth.json` with it, so other running copies never read a half-written file and a crash keeps the previous sign-in. Running copies renew one at a time, taking turns through a lock on `~/.rust-claude/auth.lock`. Before renewing, each checks `auth.json` for a sign-in another running rust-claude has already renewed, and uses that one, so several running copies stay signed in. If renewing fails, it checks `auth.json` again. At start, it asks you to sign in again only when the server turns the sign-in down or `auth.json` is not valid. If it cannot reach the server, or the server has a problem, it stops with that error and keeps the saved sign-in for the next run. During a session, it tries renewing again as described in [Retries](#retries). Only when the server turns the sign-in down does the prompt fail and ask you to restart rust-claude to sign in again.

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

In print mode, tool calls go to standard error, with syntax highlighting when standard error is a terminal. rust-claude removes control characters other than newline and tab, such as escape sequences that clear the screen or set the clipboard, from the tool calls, tool errors, notices and MCP messages it shows, and from the answer when standard output is a terminal. A piped answer is written byte for byte.

Press Ctrl+C in print mode to stop. rust-claude stops running commands and MCP servers, saves the session with the prompt marked as cancelled, waits up to 5 seconds for a sign-in renewal in progress to be saved, prints `cancelled` to standard error and exits with status 130. Closing the terminal (SIGHUP) or `kill` (SIGTERM) stops it the same way, with status 129 or 143. In the interactive mode they quit as if you had pressed Ctrl+D, so running commands and MCP servers are stopped too. While rust-claude asks for the sign-in code or renews the sign-in at start, they stop it in both modes: it waits up to 5 seconds for the renewal to be saved, prints `cancelled` and exits with status 130, 129 or 143.

Choose the model and effort level for one run with `--model <id>` and `--effort <level>`. See [Settings](#settings).

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

Sessions hold whole conversations and tool output, so rust-claude creates `~/.rust-claude` and the folders and files it writes there so that only you can read them. Folders and files made by older versions keep their permissions. If `settings.json` or `auth.json` is a symlink, for example into a dotfiles folder, saving keeps the symlink and replaces the file it points to.

Show help with `-h` or `--help`. An unknown option, or an argument that is not valid UTF-8 text, stops rust-claude with the usage line.

The interface starts with the name and version, such as `rust-claude v0.1.0`, above the start-up lines. Builds from `main` show `v0.0.0`.

Markdown tables in replies wrap their cells to fit the window width.

Edit tool calls show a diff, and write tool calls show the first 10 lines of the new file. Both use syntax highlighting based on the file extension. Removed lines have a red background and added lines have a green background. To save room, indentation is shown as one space per level, with each tab counting as one level. Python tool calls show all their code with Python syntax highlighting, using the same compact indentation.

## Commands

| Command | Action |
| --- | --- |
| `/compact` | Summarise the conversation to free context |
| `/context` | Show what fills the context |
| `/model [id]` | Select a model. The picker shows the latest model in each class. An id must be in the list of available models |
| `/new` | Start a new session |
| `/quit` | Quit |
| `/resume` | Resume a previous session |
| `/skill:name [request]` | Run a skill, with an optional request |
| `/effort [level]` | Select an effort level |

Typing `/` lists the commands and skill commands that start with the input, in alphabetical order. A command typed out in full comes first. The list shows up to 10 at a time, and a longer list scrolls as you move through it, with a scrollbar on the right. Up on the first item moves to the last, and Down on the last moves to the first.

An unknown command or effort level shows an error and leaves the text in the input, so you can edit it.

The `/resume`, `/model` and `/effort` pickers open in a box above the input. The box shows up to 10 items, and the conversation stays visible above it. A longer list scrolls as you move through it, with a scrollbar on the right. Esc or Ctrl+C closes a picker and keeps the text in the input.

## Files

Type `@` to pick a file. The list shows up to 10 project files whose path contains the text after `@`, ignoring case. A path equal to that text comes first, then paths that start with it, then the rest, so Enter on a full path such as `@src/lib.rs` picks that file. Use Up / Down to select, and Tab or Enter to insert `@path` into the prompt. Esc closes the list. In a Git repository the list holds tracked files and untracked files that `.gitignore` does not exclude. Otherwise it holds up to 10,000 files, except hidden ones, those in `target` and those that `.gitignore` or `.ignore` files exclude. rust-claude reads the files in the background when you first type `@` after each prompt, so typing never waits for it.

## Prompt history

Press Ctrl+R to search earlier prompts. The search opens in a box above the input, with the conversation still visible above it. The box shows up to 10 prompts, with a scrollbar when there are more. Type to filter the list, ignoring case. The list has two tabs: **Folder** holds prompts sent from the current folder, and **All** holds prompts from every folder, with the folder name next to each one. Left and Right switch tabs, Up / Down select, Enter puts the prompt in the input to edit, and Esc, Ctrl+R or Ctrl+C closes the list and keeps the text in the input. Repeated prompts show once, at their newest position.

In the interface, rust-claude saves each prompt you send, including `!` commands, skill commands and queued prompts, to `~/.rust-claude/history.jsonl` with the folder it was sent from. The file keeps the newest 10,000 prompts; rust-claude drops older ones when it starts, and keeps prompts that other running copies send meanwhile. It reads the file once at start-up: the **All** tab holds the prompts from the file and those you send in this run, but not those sent from another rust-claude since it started. Up / Down and the **Folder** tab hold the prompts sent from the current folder, so they keep them after a restart or `/new`. Resuming a session, with `/resume` or `--continue`, also adds its prompts to Up / Down and the **Folder** tab for that run. Prompts sent in print mode or before this file existed are not in it.

## Keys

| Key | Action |
| --- | --- |
| Enter | Send prompt, or queue it while the agent works |
| Shift+Enter / Alt+Enter | Add a new line |
| Esc | Cancel the running prompt or model lookup, or quit when idle |
| Ctrl+C | Clear input, or quit when input is empty. In a picker or prompt search, close it |
| Ctrl+D | Quit when input is empty |
| Ctrl+V | Paste an image from the clipboard |
| Ctrl+R | Search prompt history |
| Shift+Tab | Cycle effort level |
| Ctrl+P | Switch to the next model: Fable, Opus, Sonnet, Haiku |
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

The status line is a single line below the input. It shows the folder and Git branch, the model and effort level, context use, and token use for the session: input (↑), output (↓), cache reads (R), cache writes (W), the cache hit rate (CH) and the average output speed in tokens per second (tps). A `·` separates each part. It wraps when the terminal is too narrow. The folder and branch update in the background after each `bash` or `python` tool call and `!` command. Context use shows as a percentage of the context window, for example `25%/200k`, or as a token count for a model with no known context window.

It also shows how much of the 5-hour and 7-day quota is left and when each resets. rust-claude loads it in the background at start, so you can send a prompt straight away, and updates it after each reply. When only one of them is known, it is labelled, for example `5h 95% 2h16m` or `7d 81% 2d12h`.

## Settings

| Setting | Option | `~/.rust-claude/settings.json` key | Default |
| --- | --- | --- | --- |
| Model | `--model <id>` | `model` | `claude-opus-5-5` |
| Effort level | `--effort <level>` | `effort` | `medium` |
| Check for updates | | `check_for_updates` | `true` |

Effort levels: `off`, `low`, `medium`, `high`, `xhigh`, `max`.

`off` turns thinking off and is offered only on Sonnet and Haiku, because Fable and Opus cannot turn thinking off. On Sonnet it turns off thinking before the reply, but Claude still writes short updates between tool calls. With `off`, rust-claude sends no effort level, so the model uses its default. Switching to Fable or Opus, or starting with them, changes `off` to `medium`.

rust-claude knows only the latest model in each class: `claude-fable-5-1`, `claude-opus-5-5`, `claude-sonnet-5-5` and `claude-haiku-5-5` (see [ADR 4](docs/adr/0004-offer-only-the-latest-model-in-each-class.md)). Every model uses adaptive thinking unless the effort level is `off`, and the effort level controls how much work Claude puts into each reply, thinking included. rust-claude no longer reads the old `thinking_level` key.

Each reply from a known model may use up to 128,000 tokens. Other models get 8,192 tokens, and context use shows as a token count.

Options take priority over the settings file and apply to that run only. They work in the interface and in print mode. rust-claude does not check the `--model` id, so an unknown id fails on the first request.

An unknown effort level prints a warning to standard error and uses `medium`.

Changing the model or effort level in the interface saves only the setting you changed. If `settings.json` is not valid JSON, rust-claude uses the defaults, and changing a setting shows an error instead of replacing the file, so fix or delete it first.

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

Type `/skill:name` to run a skill yourself. It sends the skill's instructions to the model, followed by any text after the name. That text can start on the same line or on the next one. The conversation shows `[skill] name` in place of the instructions. Tab completes skill commands and marks each one as `[global]` or `[project]`. A command typed out in full comes first in the list, so `/skill:pdf` and Enter runs `pdf` even when `pdf-tools` also matches.

The skill name comes from `name` in the frontmatter, or else from the folder that holds `SKILL.md`, or from the file name without `.md` for other Markdown files. Names should use lowercase letters, numbers and single hyphens, up to 64 characters. Descriptions can have up to 1,024 characters. A skill without a description does not load. If two skills share a name, the first one found wins. At start, the interface lists the loaded global skills, then the loaded project skills, and shows a warning for each problem. Global skills come from your home folder (places 3 and 4). Project skills come from the current directory and its parents (places 1 and 2).

## Retries

If a request fails with a rate limit (429), an overloaded API (529), a server error (5xx) or a connection error before the reply starts, rust-claude tries again up to 3 times. It waits 1, 2 and then 4 seconds, or as long as the `retry-after` header asks. It does not retry when `retry-after` is longer than 60 seconds. A notice shows each retry. Tokens that a failed attempt already used still count toward the session's token use, but not toward context use, the cache hit rate or the output speed.

A request counts as a connection error when it cannot connect within 30 seconds, or when no data arrives for 5 minutes. If no data arrives after the reply has started, the prompt fails with an error instead of hanging.

Renewing the sign-in before a request is retried the same way when it fails for any reason other than the server turning the sign-in down.

If the API turns the sign-in down before it expires, for example because it was revoked, rust-claude renews it and sends the request once more. If the server also turns the renewal down, the prompt fails with an error that asks you to restart rust-claude, and the next start asks you to sign in again.

## Images

Press Ctrl+V to paste an image from the clipboard. It adds a marker such as `[image 1]` at the cursor. Only images whose marker is still in the prompt are sent, so deleting the marker removes the image. On macOS, rust-claude reads the clipboard with `osascript`. On Linux, it uses `wl-paste` under Wayland and `xclip` otherwise. If one of these does not answer within 5 seconds, rust-claude stops it and shows an error.

rust-claude sends PNG, JPEG, GIF and WebP images as they are when they fit the limits. It scales larger images down to at most 2,000 pixels on the long edge, and converts other formats to PNG. Photos stored sideways with an Exif orientation, as phone cameras often save them, are turned upright and re-encoded. If an image is still larger than 5 MB in base64, it uses JPEG at lower quality, then halves the size until it fits. JPEG has no transparency, so transparent areas turn white.

An image that would take more than 512 MiB of memory to decode, such as a small file that claims to be tens of thousands of pixels wide, is refused with the error `image is too large`.

## Sessions

Sessions are saved to `~/.rust-claude/sessions/` as JSON Lines files. The first line records the folder the session started in, which `--continue` uses. Sessions saved before this line was added are not found by `--continue`, but `/resume` still lists them. Images sent with a prompt are saved in a folder named after the session, such as `~/.rust-claude/sessions/<id>/`, and the session file refers to them by name.

While a prompt runs, the session is saved after each finished round of tool calls, so a crash or a closed terminal loses at most the round in progress. If a crash or a full disk cuts a save short, resuming the session keeps the messages before the cut, leaves out tool calls whose results were cut off, and removes the rest from the file.

A session is open in one rust-claude at a time, so two copies never write to the same file. Resuming a session that another running rust-claude has open, with `/resume` or `--continue`, shows an error instead.

Quitting while a prompt runs cancels it and saves the session first. Quitting while `/model` loads the model list stops that request. Requests still waiting when you quit, such as a prompt sent just before, are not started. Quitting waits at most a second for background work, such as listing files for `@`, reading the Git branch or pasting an image.

A cancelled or failed prompt stays in the session with its finished tool calls, so the model sees them in the next request. This includes a prompt cancelled before it was sent, for example while the sign-in renews at start. A tool call that was still running or waiting when you cancelled is recorded as cancelled.

If the model declines to answer, even partway through a reply, the prompt fails with the error `the model declined to answer`. The partial reply is left out of the session and is not sent again, but its tokens still count toward the session's token use.

If a reply stops early because it reached the output limit or filled the context window, a notice says the reply was cut off and why.

Newer models only use thinking from earlier replies while the system prompt, tools and messages sent before it stay the same. These can change during a session, for example when an MCP server connects after the first prompt, or when a resumed session finds different `AGENTS.md` files or skills. When the API rejects a request for this reason, rust-claude shows a notice, sends the request once more asking the API to drop the thinking that no longer matches, and keeps asking for the rest of the session. The session file records this, so it also applies after resuming.

`/compact` asks the model to summarise the conversation. Later requests send the summary in place of the earlier messages. The session file keeps the full conversation and a compaction entry that holds the summary. A resumed session shows the earlier messages, then a `compacted conversation` notice. Esc cancels a running compaction. If the summary is cut off, or the model declines to write it, compaction fails with an error and the conversation stays as it was.

rust-claude also compacts automatically when the context is 80% full: before sending a new prompt, and after each round of tool calls. A prompt sent at that point, or queued during that round, is kept word for word after the summary. Models that rust-claude doesn't know have no known context window, so they only compact with `/compact`.

A round of tool calls can take the conversation past the context window, and the API then refuses to read it. When that happens during compaction, rust-claude shows a notice and asks for the summary again with each tool output cut to its first 2,000 characters, then to 200. The session file still keeps the full output.

`/context` shows how many tokens each part of the context uses: the system prompt, instructions, skills, built-in tools, MCP tools and messages, and how much of the context window is free. Each part is estimated from its length, then scaled so that the parts add up to the context use on the status line. That figure comes from the token count of the last reply, plus an estimate for anything added since. Images are not counted.

`/resume` reads each session only up to its first prompt to build the list. A session that starts with a `!` command shows that command, such as `!ls`, as its preview, and one that starts with a skill command shows it as typed, such as `/skill:demo fix it`. A session that is damaged before its first prompt is left out of the list. Resuming a session that is damaged later in the file keeps the messages before the damage, as after a crash. A resumed session shows messages, tool calls and failed tool calls as they appeared live.

## Tools

The agent can use these tools:

- `bash`: run a shell command
- `python`: run Python code with `python3`, which must be on `PATH`. The code goes in on standard input
- `read`: read a file. It reads only up to the requested lines and stops once it has 20,000 bytes of output, so the first lines of a huge file or a pipe come back straight away
- `write`: create or replace a file
- `edit`: replace text in a file. The text must match exactly once, unless `replace_all` is true, which replaces every match and reports how many replacements it made. The count shows on a line such as `edit: 3 replacements`, in the interface, in resumed sessions and in print mode. In a file with Windows line endings (CRLF), it also matches text written with plain line endings, and writes the new text with CRLF line endings. A file counts as using CRLF when its first line ends with CRLF. It refuses files larger than 10 MiB and anything that is not a regular file, such as a pipe or device

The agent can also use tools from [MCP servers](#mcp-servers).

`write` and `edit` write a temporary file in the same folder and then replace the file with it, so a failed write, for example on a full disk, keeps the old file. The file keeps its permissions and group, so a script stays executable, and a symlink stays a symlink, with the file it points to replaced. A file with other hard links, or owned by someone else, is written in place instead, so every link sees the new text and the owner does not change. `write` to a pipe or device sends the text into it and leaves it in place.

`bash` keeps only the first 20,000 bytes of output and discards the rest as it arrives. It returns once the command exits, even if a background process it started keeps running. If a command times out, `bash` returns the output so far, followed by the timeout notice. `bash` and `!` commands and MCP servers run without the terminal, so programs that prompt on it, such as `sudo`, `ssh` or `git` asking for a password, fail at once instead of waiting for input. `python` works the same way as `bash`.

Consecutive `read` calls show as one line with the paths separated by commas, for example `read src/main.rs (2), README.md`. A number in brackets shows how many times a file was read. A read with `offset` or `limit` shows its line range, for example `src/tools.rs:325-354`, or `src/tools.rs:325-` when only `offset` is given. A failed read starts a new line. In print mode, the line is printed when the next tool, text or notice arrives.

## MCP servers

rust-claude connects to [Model Context Protocol](https://modelcontextprotocol.io) servers over stdio or Streamable HTTP and gives their tools to the model. Add servers to `~/.rust-claude/mcp.json`, or to `.rust-claude/mcp.json` in a project. The format uses the standard `mcpServers` shape:

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
- `url` is the address of a Streamable HTTP server and must start with `http://` or `https://`. `headers` sets extra request headers, such as `Authorization`. `${NAME}` in a header value is replaced with the environment variable `NAME`; the server fails to connect if it is not set. A project `mcp.json` can use this too, so it can send your environment variables to a server it chooses. If connecting fails because the server cannot be reached or answers with status 408, 429 or 5xx (other than 501), rust-claude tries twice more, after 0.25 and 1 second. rust-claude keeps the session the server gives it and ends the session when it quits. If the server says the session has expired (status 404), rust-claude starts a new one and sends the request once more. A server without an `Authorization` header signs in with OAuth; see [Signing in to MCP servers](#signing-in-to-mcp-servers).
- `oauth` sets how rust-claude signs in to an HTTP server. `clientId` and `clientSecret` name a client you registered yourself, so rust-claude does not register; `${NAME}` in `clientSecret` is replaced with the environment variable `NAME`.
- `timeout` sets the time limit for each request in seconds, at least 1 (default 60). When a request times out, or you press Esc during a tool call, rust-claude tells the server to stop working on it.
- `enabled: false` keeps an entry without connecting to it.
- `type` is optional. When present, it must be `stdio`, `http` or `streamable-http`. Without `type`, an entry with `url` and no `command` is an HTTP server. The older SSE transport (`sse`) is not supported; use the server's Streamable HTTP URL.
- Server names may only contain letters, digits, `_` and `-`.

Project entries replace global entries with the same name. rust-claude starts project servers without asking (see [ADR 3](docs/adr/0003-start-project-mcp-servers-without-approval.md)). When you run rust-claude from your home folder, `~/.rust-claude/mcp.json` counts as global only. A missing `mcp.json` is fine; one that exists but cannot be read gives a notice.

In the interface, rust-claude starts all servers in the background, so you can type and send prompts straight away. It lists the servers on two lines, one for global servers and one for project servers, with a spinner on each server still loading, such as `project MCP servers: ⠋ docs, web (2 tools)`. Each server's tools become available once it is ready, and its entry then changes to its tool count, such as `docs (3 tools)`, or to `docs (failed)` or `docs (needs sign-in)`. Once every server on a line is ready, the line starts with `loaded`, such as `loaded project MCP servers: docs (3 tools), web (2 tools)`. A global server that a project entry overrides stays on the global line as `docs (overridden)`. The reason a server failed shows on its own line. A prompt sent before then goes without those tools, and a server that becomes ready while a prompt runs joins after that prompt. The interface also shows a notice for each invalid entry. In print mode, rust-claude waits for all servers before sending the prompt, and the notices for invalid entries and failed servers go to standard error. Tools are named `mcp__<server>__<tool>`, with other characters replaced by `_` and cut to 64 characters. `anyOf`, `oneOf` and `allOf` at the top level of a tool's input schema are dropped, because the API rejects them. Text results longer than 20,000 bytes are cut. Images, audio and binary resources show as short placeholders. Servers stop when rust-claude quits.

### Signing in to MCP servers

An HTTP server without an `Authorization` header in its entry signs in with OAuth. When it needs sign-in, its entry shows `figma (needs sign-in)`, and a line below shows `MCP server figma needs sign-in: run /mcp login figma`.

Type `/mcp login <server>` in the interface. After `/mcp `, the command list offers `login` and `logout`, and after either one the names of the HTTP servers that sign in with OAuth. rust-claude finds the server's sign-in server, registers with it unless `oauth.clientId` is set, and opens the sign-in page in your browser (`open` on macOS, `xdg-open` elsewhere). It also shows the link, in case the browser does not open. The page sends the browser back to `http://127.0.0.1:<port>/callback`, where rust-claude waits for up to 5 minutes. rust-claude then starts the server again with the new sign-in, and its entry shows the spinner again. You can keep working while it waits.

rust-claude saves the sign-in for each server URL in `~/.rust-claude/mcp-auth.json`, which only you can read. It renews the sign-in when it expires or the server turns it down. `/mcp logout <server>` removes the saved sign-in and starts the server again without it. In print mode, servers use the saved sign-in, but you can only sign in from the interface.

rust-claude registers as `Claude Code`, because some sign-in servers only let approved apps register. Figma, for example, turns down `rust-claude` but accepts `Claude Code`. Using another app's name may break the server's terms (see [ADR 1](docs/adr/0001-identify-as-claude-code.md)).

## Decisions

Architecture decision records are in [docs/adr](docs/adr):

- [1. Identify as Claude Code](docs/adr/0001-identify-as-claude-code.md)
- [2. Run tools without approval](docs/adr/0002-run-tools-without-approval.md)
- [3. Start project MCP servers without approval](docs/adr/0003-start-project-mcp-servers-without-approval.md)
- [4. Offer only the latest model in each class](docs/adr/0004-offer-only-the-latest-model-in-each-class.md)

## Development

Before each commit, run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`. Tests keep sign-in, sessions and other files in a temporary folder, not in `~/.rust-claude`.

Every push to `main` runs these checks and then makes a release if needed. [git-cliff](https://git-cliff.org) works out the next version from the [conventional commit](https://www.conventionalcommits.org) messages since the last tag: `fix` and `perf` bump the patch version, `feat` the minor version and a breaking change (`feat!` or `BREAKING CHANGE`) the major version. Commits of other types, such as `docs` or `chore`, do not make a release. The release adds tag `v<version>` and a `rust-claude.zip` with the source and the version set in `Cargo.toml`. Nothing is pushed to `main`, so `Cargo.toml` there stays at `0.0.0`.

## Licence

MIT. See [LICENSE](LICENSE).
