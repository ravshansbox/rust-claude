# Usage

## Command line

Start the interactive interface:

```sh
rca
```

Run one prompt and print the answer with `--print`. The argument after it is always the prompt, even when it starts with `-`:

```sh
rca --print "explain src/main.rs"
```

Without `--print`, when standard input is not a terminal, rca reads the prompt from it and runs in print mode. Trailing white space is removed, and an empty prompt is an error, as it is with `--print`. This lets you run the same prompt in a loop, each time in a new session:

```sh
while :; do cat PROMPT.md | rca || break; done
```

`--loop` does the same in one run. It sends the prompt from `--print` or standard input again and again, each time in a new session, until a run fails or you stop it. Before each run it shows `loop 1`, `loop 2` and so on on standard error. With `--continue` or `--resume`, the first run continues that session. Images from `--image` go with every run. Without a prompt from `--print` or standard input, `--loop` is an error:

```sh
rca --loop --print "pick the next task from TODO.md, do it and commit"
```

In print mode, `/skill:name` runs a skill as in the interface. Other commands, such as `/compact`, are not available there; their text is sent as the prompt.

Send images with the prompt with `--image <path>`. Repeat it for more images:

```sh
rca --print "what is wrong in this screenshot?" --image error.png
```

In print mode, tool calls and thinking go to standard error. When standard error is a terminal, they look as they do in the interface, with the same colours and syntax highlighting, and thinking is grey and in italics. A blank line separates each tool call, thinking block, notice and part of the answer. The interface does the same, except that notices in a row, such as the lines shown at start, have no blank lines between them. When standard output and standard error are both a terminal, these blank lines go to standard error, so standard output holds only the answer. rust-coding-agent removes control characters other than newline and tab, such as escape sequences that clear the screen or set the clipboard, from the tool calls, tool errors, notices and MCP messages it shows, and from the answer when standard output is a terminal. A piped answer is written byte for byte.

Press Ctrl+C in print mode to stop. rust-coding-agent stops running commands and MCP servers, saves the session with the prompt marked as cancelled, waits up to 5 seconds for a sign-in renewal in progress to be saved, prints `cancelled` to standard error and exits with status 130. Closing the terminal (SIGHUP) or `kill` (SIGTERM) stops it the same way, with status 129 or 143. In the interactive mode they quit as if you had pressed Ctrl+D, so running commands and MCP servers are stopped too. While rust-coding-agent asks for the sign-in code or renews the sign-in at start, they stop it in both modes: it waits up to 5 seconds for the renewal to be saved, prints `cancelled` and exits with status 130, 129 or 143.

Choose the model and effort level for one run with `--model <id>` and `--effort <level>`. See [Settings](configuration.md#settings).

Continue the latest session started in the current folder with `--continue`. It works in the interface and in print mode:

```sh
rca --continue
rca --continue --print "now add tests"
```

If no session in the folder can be continued, rust-coding-agent stops with an error.

Continue a given session with `--resume <id>`. The id is the name of the session file in `~/.rust-coding-agent/sessions/` without `.jsonl`. It works in the interface and in print mode, and cannot be used with `--continue`. If there is no session with that id, rust-coding-agent stops with an error.

Keep sign-in, settings, sessions, prompt history, skills and MCP config in another folder with `--config-dir <path>`. It works in the interface and in print mode. Wherever these docs mention `~/.rust-coding-agent`, it means this folder when the option is given. `~/AGENTS.md` and `~/.agents/skills/` still come from your home folder.

```sh
rca --config-dir ~/work/.rust-coding-agent
```

If `~/.rust-coding-agent` does not exist but `~/.rust-claude` from before the rename does, rust-coding-agent moves it to `~/.rust-coding-agent` at start, unless `--config-dir` is given. Project `.rust-claude` folders are not moved; rename them to `.rust-coding-agent` yourself.

Sessions hold whole conversations and tool output, so rust-coding-agent creates `~/.rust-coding-agent` and the folders and files it writes there so that only you can read them. Folders and files made by older versions keep their permissions. If `settings.json` or `auth.json` is a symlink, for example into a dotfiles folder, saving keeps the symlink and replaces the file it points to.

Show help with `--help`. An unknown option, or an argument that is not valid UTF-8 text, stops rust-coding-agent with the usage line.

The interface starts with the name and version, such as `Rust Coding Agent v0.1.0`, above the start-up lines. Builds from `main` show `v0.0.0`.

Markdown tables in replies wrap their cells to fit the window width.

In the interface and in print mode, each tool call has a background that fills the row. Its colour is the terminal's background made slightly lighter on a dark theme or slightly darker on a light one. If the terminal does not report its background, a light or dark grey is used instead. The tool name is in bold. Edit tool calls show a diff, and write tool calls show the first 10 lines of the new file. Both use syntax highlighting based on the file extension. Removed lines have a red background and added lines have a green background. To save room, indentation is shown as one space per level, with each tab counting as one level. Bash tool calls show the first line of their command next to the tool name and any further lines below it, with Bash syntax highlighting. Python, Node and Ruby code inside a Bash command, in a heredoc (`python3 - <<'EOF'`) or after `-c` or `-e` (`python3 -c '…'`, `node -e '…'`, `ruby -e '…'`), gets that language's highlighting, with the same compact indentation.

Syntax highlighting in tool calls and in code blocks in replies uses the GitHub High Contrast colours from [github-nvim-theme](https://github.com/projekt0n/github-nvim-theme): GitHub Dark High Contrast on a dark terminal and GitHub Light High Contrast on a light one. The themes are in [assets/themes](../assets/themes).

## Commands

| Command | Action |
| --- | --- |
| `/compact` | Summarise the conversation to free context |
| `/loop` | Turn loop mode on or off. See [Loop mode](#loop-mode) |
| `/mcp login <server>` | Sign in to an MCP server. See [Signing in to MCP servers](mcp.md#signing-in-to-mcp-servers) |
| `/mcp logout <server>` | Remove the saved sign-in for an MCP server and start it again |
| `/model [id]` | Select a model. The id must be one of the four models rust-coding-agent knows. The picker and status line show the model name without the version, for example `opus` |
| `/new` | Start a new session |
| `/quit` | Quit |
| `/resume` | Resume a previous session |
| `/skill:name [request]` | Run a skill, with an optional request |
| `/effort [level]` | Select an effort level |

Typing `/` lists the commands and skill commands that start with the input, in alphabetical order. A command typed out in full comes first. The list shows up to 10 at a time, and a longer list scrolls as you move through it, with a scrollbar on the right. The list has the same background as tool calls. Up on the first item moves to the last, and Down on the last moves to the first.

An unknown command, effort level or skill shows an error and leaves the text in the input, so you can edit it.

The `/resume`, `/model` and `/effort` pickers open in a box above the input. The box shows up to 10 items, and the conversation stays visible above it. A longer list scrolls as you move through it, with a scrollbar on the right. Esc or Ctrl+C closes a picker and keeps the text in the input.

### Loop mode

`/loop` turns loop mode on and shows `loop mode on`. The next prompt you send, including a skill command and its images, then runs again and again, each time in a new session. The first run uses the current session. After each run, once no background command or queued prompt is waiting, the screen clears and the prompt goes out again in a new session. The status line shows `loop` while loop mode waits for a prompt, then the run number, for example `loop 3`.

The loop stops, and loop mode turns off with `loop mode off`, when a run fails or you press Esc. `/loop` turns it off too, even while a run is going; that run then finishes and no other starts. While loop mode is on, rust-coding-agent does not restart into an installed update. For the same loop in print mode, see `--loop` under [Command line](#command-line).

## Files

Type `@` to pick a file. The list shows up to 10 project files whose path contains the text after `@`, ignoring case. A path equal to that text comes first, then paths that start with it, then the rest, so Enter on a full path such as `@src/lib.rs` picks that file. Use Up / Down to select, and Tab or Enter to insert `@path` into the prompt. Only the path is sent, not the file; the model reads it with `read` if it needs to. Esc closes the list. In a Git repository the list holds tracked files and untracked files that `.gitignore` does not exclude. Otherwise it holds up to 10,000 files, except hidden ones, those in `target` and those that `.gitignore` or `.ignore` files exclude. rust-coding-agent reads the files in the background when you first type `@` after each prompt, so typing never waits for it.

After `@~/`, `@/`, `@./` or `@../`, the list shows the entries of the folder typed so far instead, up to 10, whose names start with the text after the last `/`, ignoring case. `~` is your home folder. Folders end in `/`, and picking one inserts it without a space so you can keep going down. Hidden entries show only when the name you type starts with `.`. The path is inserted as typed, so `~` is kept.

## Prompt history

Press Ctrl+R to search earlier prompts. The search opens in a box above the input, with the conversation still visible above it. The box shows up to 10 prompts, with a scrollbar when there are more. Type to filter the list, ignoring case. The list has two tabs: **Folder** holds prompts sent from the current folder, and **All** holds prompts from every folder, with the folder name next to each one. Left and Right switch tabs, Up / Down select, Enter puts the prompt in the input to edit, and Esc, Ctrl+R or Ctrl+C closes the list and keeps the text in the input. Repeated prompts show once, at their newest position.

In the interface, rust-coding-agent saves each prompt you send, including `!` commands, skill commands and queued prompts, to `~/.rust-coding-agent/history.jsonl` with the folder it was sent from. The file keeps the newest 10,000 prompts; rust-coding-agent drops older ones when it starts, and keeps prompts that other running copies send meanwhile. It reads the file once at start-up: the **All** tab holds the prompts from the file and those you send in this run, but not those sent from another rust-coding-agent since it started. Up / Down and the **Folder** tab hold the prompts sent from the current folder, so they keep them after a restart or `/new`. Resuming a session, with `/resume` or `--continue`, also adds its prompts to Up / Down and the **Folder** tab for that run. Prompts sent in print mode or before this file existed are not in it. It saves prompts in the background, so a prompt goes out straight away even while another running copy is writing to the file.

## Keys

| Key | Action |
| --- | --- |
| Enter | Send prompt, or queue it while the agent works |
| Shift+Enter / Alt+Enter | Add a new line |
| Esc | Cancel the running prompt, or quit when idle |
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

While a reply streams, the view follows new text only when it is scrolled to the bottom. If you scroll up, the view stays where it is, and the spinner and queued prompts stay pinned above the input. A link whose address comes later in the reply shows as plain text until the reply finishes.

Shift+Enter works in terminals that support the kitty keyboard protocol, such as kitty, WezTerm, Ghostty and iTerm2. Other terminals send it as Enter, so use Alt+Enter there.

## Input

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

## Display

- Your prompts shown on grey rows
- Markdown replies, with tables wrapped to the window width
- Inline code in replies shown in blue
- Tabs in replies, tool output and `!` command output shown as four spaces
- Light and dark theme detection
- Mouse wheel scrolling
- When scrolled up, a `↓ Bottom (End)` button above the prompt, on the right of the spinner line; click it to jump to the bottom
- Drag over the conversation to quote text. Releasing the button adds it to the input at the cursor as `'text' - `, with a space before it if needed. Each screen row stays on its own line
- To copy text, use your terminal's own selection by holding a key while you drag: Shift in most terminals, Option in iTerm2
- The view stays still while you read earlier text and a reply streams
- Spinner with status text
- Terminal notification when a run ends while the window is not in focus (see [Notifications](#notifications))

## Shell commands

Start a prompt with `!` to run the rest as a shell command, for example `!git status`. The command runs in the current directory, and its output shows in the conversation. It is also added to the conversation, so the model sees the command and its output with your next prompt. It is not sent on its own.

Output is capped at 20,000 bytes, as with the `bash` tool. The command has no time limit; press Esc to stop it. Resumed sessions show these commands and their output.

## Queued prompts

While the agent works, Enter queues the prompt instead of sending it. Queued prompts show below the spinner. After the next round of tool calls, rust-coding-agent adds them to the conversation, so the model reads them before it carries on. If the reply ends first, they go out together as a new prompt. Commands such as `/new`, skill commands and `!` commands are not queued.

If you cancel with Esc, or the prompt fails, the queued prompts go back into the input so you can edit or resend them.

## Status line

The status line is a single line below the input. It shows the folder and Git branch, the model name and effort level (for example `opus:medium`), `loop` or the run number (for example `loop 3`) while [loop mode](#loop-mode) is on, the number of subagents running while any run (for example `2 agents`), the number of [background commands](tools.md#background-commands) running or waiting to be answered (for example `1 background`), context use, and stats for the session: input tokens (↑), output tokens (↓), cache reads (R), cache writes (W) and the average output speed in tokens per second (t/s), for example `↑4 ↓338 R6k W88 42t/s`. A `·` separates each part. It wraps when the terminal is too narrow. The folder and branch update in the background after each `bash` tool call and `!` command. Context use shows as a percentage of the context window, for example `25%/200k`.

It also shows how much of the 5-hour and 7-day quota is left and when each resets. rust-coding-agent loads it in the background at start, so you can send a prompt straight away, and updates it after each reply. When only one of them is known, it is labelled, for example `5h 95% 1h16m` or `7d 81% 3d`. When the first unit is 2 or more, only that unit is shown, rounded to the nearest, so `2h40m` shows as `3h`.

## Notifications

When a run finishes or fails while the terminal window is not in focus, rust-coding-agent sends a terminal notification (OSC 9), for example `Done` or `Failed: overloaded`. Cancelled runs do not notify, and nor does print mode. Terminals that support OSC 9 include iTerm2, Ghostty, WezTerm, kitty and Windows Terminal; other terminals ignore it. Inside tmux, the notification goes through tmux passthrough, so set `allow-passthrough` to `on` in tmux, or to `all` to get notifications from panes that are not visible. Set `notifications` to `false` in `settings.json` to turn it off.

## Images

Press Ctrl+V to paste an image from the clipboard. It adds a marker such as `[image 1]` at the cursor. Only images whose marker is still in the prompt are sent, so deleting the marker removes the image. On macOS, rust-coding-agent reads the clipboard with `osascript`. On Linux, it uses `wl-paste` under Wayland and `xclip` otherwise. If one of these does not answer within 5 seconds, rust-coding-agent stops it and shows an error.

rust-coding-agent sends PNG, JPEG, GIF and WebP images as they are when they fit the limits. It scales larger images down to at most 2,000 pixels on the long edge, and converts other formats to PNG. Photos stored sideways with an Exif orientation, as phone cameras often save them, are turned upright and re-encoded. If an image is still larger than 5 MB in base64, it uses JPEG at lower quality, then halves the size until it fits. JPEG has no transparency, so transparent areas turn white.

An image that would take more than 512 MiB of memory to decode, such as a small file that claims to be tens of thousands of pixels wide, is refused with the error `image is too large`.

## Sessions

Sessions are saved to `~/.rust-coding-agent/sessions/` as JSON Lines files. The first line records the folder the session started in, which `--continue` uses. Sessions saved before this line was added are not found by `--continue`, but `/resume` still lists them. Images sent with a prompt or read by the `read` tool are saved in a folder named after the session, such as `~/.rust-coding-agent/sessions/<id>/`, and the session file refers to them by name.

While a prompt runs, the session is saved after each finished round of tool calls, so a crash or a closed terminal loses at most the round in progress. If a crash or a full disk cuts a save short, resuming the session keeps the messages before the cut, leaves out tool calls whose results were cut off, and removes the rest from the file.

A session is open in one rust-coding-agent at a time, so two copies never write to the same file. Resuming a session that another running rust-coding-agent has open, with `/resume` or `--continue`, shows an error instead.

Quitting while a prompt runs cancels it and saves the session first. Requests still waiting when you quit, such as a prompt sent just before, are not started. Quitting waits at most a second for background work, such as listing files for `@`, reading the Git branch or pasting an image.

A cancelled or failed prompt stays in the session with its finished tool calls, so the model sees them in the next request. This includes a prompt cancelled before it was sent, for example while the sign-in renews at start. A tool call that was still running or waiting when you cancelled is recorded as cancelled.

If the model declines to answer, even partway through a reply, the prompt fails with the error `the model declined to answer`. The partial reply is left out of the session and is not sent again, but its tokens still count toward the session's token use.

If a reply stops early because it reached the output limit or filled the context window, a notice says the reply was cut off and why.

Newer models only use thinking from earlier replies while the system prompt, tools and messages sent before it stay the same. These can change during a session, for example when an MCP server connects after the first prompt, or when a resumed session finds different `AGENTS.md` files or skills. When the API rejects a request for this reason, rust-coding-agent shows a notice, sends the request once more asking the API to drop the thinking that no longer matches, and keeps asking for the rest of the session. The session file records this, so it also applies after resuming.

`/compact` asks the model to summarise the conversation. Later requests send the summary in place of the earlier messages. The session file keeps the full conversation and a compaction entry that holds the summary. A resumed session shows the earlier messages, then a `compacted conversation` notice. Esc cancels a running compaction. If the summary is cut off, or the model declines to write it, compaction fails with an error and the conversation stays as it was.

rust-coding-agent also compacts automatically when the context is 80% full: before sending a new prompt, and after each round of tool calls. A prompt sent at that point, or queued during that round, is kept word for word after the summary. If automatic compaction fails, rust-coding-agent shows the error as a notice and sends the request without compacting.

A round of tool calls can take the conversation past the context window, and the API then refuses to read it. When a request is refused this way, rust-coding-agent compacts the conversation and sends the request once more. If compaction fails, the prompt fails with both errors. If the conversation was compacted just before that request, there is nothing new to compact, so the prompt fails with the API's error. When the refusal happens during compaction, rust-coding-agent shows a notice and asks for the summary again with each tool output cut to its first 2,000 characters, then to 200. The session file still keeps the full output.

Context use on the status line is the token count the API reported for the last reply. rust-coding-agent never estimates it, so it is hidden until the first reply of a session, and again after compaction until the next reply. Automatic compaction uses the same count, so tool results and prompts added since the last reply are not included until the next reply.

`/resume` reads each session only up to its first prompt to build the list. A session that starts with a `!` command shows that command, such as `!ls`, as its preview, and one that starts with a skill command shows it as typed, such as `/skill:demo fix it`. A session that is damaged before its first prompt is left out of the list. Resuming a session that is damaged later in the file keeps the messages before the damage, as after a crash. A resumed session shows messages, tool calls, failed tool calls and cancelled turns as they appeared live.
