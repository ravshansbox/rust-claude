# rust-claude

A small coding agent for the terminal, written in Rust. It talks to Claude and can run shell commands and read, write and edit files in the current directory.

## Build

```sh
cargo build --release
```

## Sign in

On first run, rust-claude prints a sign-in link for your Claude Pro or Max account and asks for the code. It saves the sign-in to `~/.rust-claude/auth.json` and renews it when it expires.

## Usage

Start the interactive interface:

```sh
rust-claude
```

Run one prompt and print the answer with `-p` or `--print`:

```sh
rust-claude -p "explain src/main.rs"
```

In print mode, tool calls go to standard error, with syntax highlighting when standard error is a terminal. Add `--hide-tools` to hide them.

Show help with `-h` or `--help`.

Markdown tables in replies wrap their cells to fit the window width.

Edit tool calls show a diff, and write tool calls show the first 10 lines of the new file. Both use syntax highlighting based on the file extension. Removed lines have a red background and added lines have a green background. Leading spaces are removed from each line to save room.

## Commands

| Command | Action |
| --- | --- |
| `/new` | Start a new session |
| `/resume` | Resume a previous session |
| `/model [id]` | Select a model. An id must be in the list of available models |
| `/thinking [level]` | Select a thinking level |
| `/quit` | Quit |

## Files

Type `@` to pick a file. The list shows up to 10 project files whose path contains the text after `@`, ignoring case. Use Up / Down to select, and Tab or Enter to insert `@path` into the prompt. Esc closes the list. In a Git repository the list holds tracked files and untracked files that `.gitignore` does not exclude. Otherwise it holds all files, except hidden ones and those in `target`.

## Keys

| Key | Action |
| --- | --- |
| Enter | Send prompt |
| Shift+Enter / Alt+Enter | Add a new line |
| Esc | Cancel the running prompt, or quit when idle |
| Ctrl+C | Clear input, or quit when input is empty |
| Ctrl+D | Quit when input is empty |
| Shift+Tab | Cycle thinking level |
| Up / Down | Browse prompt history, move between input rows, or scroll |
| Page Up / Page Down | Scroll by a page |
| Home / End | Scroll to top or bottom |
| Left / Right | Move cursor |
| Alt+Left / Alt+B, Alt+Right / Alt+F | Move by word |
| Ctrl+A / Ctrl+E | Jump to start or end of input |
| Alt+Backspace / Ctrl+W | Delete previous word |
| Tab | Complete a command or file |

While a reply streams, the view follows new text only when it is scrolled to the bottom. If you scroll up, the view stays where it is.

Shift+Enter works in terminals that support the kitty keyboard protocol, such as kitty, WezTerm, Ghostty and iTerm2. Other terminals send it as Enter, so use Alt+Enter there.

## Status line

The status line shows token use for the session: input (↑), output (↓), cache reads (R), cache writes (W), the cache hit rate (CH) and the average output speed in tokens per second (tps). A `·` separates token counts, context use and speed.

The next line shows how much of the 5-hour and 7-day quota is left and when each resets. rust-claude loads it in the background at start, so you can send a prompt straight away, and updates it after each reply.

## Settings

| Setting | Environment variable | `~/.rust-claude/settings.json` key | Default |
| --- | --- | --- | --- |
| Model | `RUST_CLAUDE_MODEL` | `model` | `claude-opus-5-5` |
| Thinking level | `RUST_CLAUDE_THINKING` | `thinking_level` | `medium` |

Thinking levels: `low`, `medium`, `high`, `xhigh`, `max`.

Each reply may use up to the model's full output limit (64,000 or 128,000 tokens). Models that rust-claude doesn't know get 8,192 tokens.

Environment variables take priority over the settings file.

An unknown thinking level prints a warning to standard error and uses `medium`.

Changing the model or thinking level in the interface saves only the setting you changed.

## Instructions

rust-claude adds `AGENTS.md` from your home folder and from the current directory to the system prompt.

The system prompt tells the model to put questions to the user in bold.
It also tells the model to prefer `edit` and `write` over `bash` for changing files.

## Retries

If a request fails with a rate limit (429), an overloaded API (529), a server error (5xx) or a connection error before the reply starts, rust-claude tries again up to 3 times. It waits 1, 2 and then 4 seconds, or as long as the `retry-after` header asks. It does not retry when `retry-after` is longer than 60 seconds. A notice shows each retry.

## Sessions

Sessions are saved to `~/.rust-claude/sessions/` as JSON Lines files.

Quitting while a prompt runs cancels it and saves the session first.

A cancelled or failed prompt stays in the session with its finished tool calls, so the model sees them in the next request.

`/resume` reads each session only up to its first prompt to build the list. A damaged session shows an error when you resume it. A resumed session shows messages, tool calls and failed tool calls as they appeared live.

## Tools

The agent can use these tools:

- `bash`: run a shell command
- `read`: read a file
- `write`: create or replace a file
- `edit`: replace text in a file. The text must match exactly once, unless `replace_all` is true, which replaces every match and reports how many replacements it made. The count shows on a line such as `edit: 3 replacements`, in the interface, in resumed sessions and in print mode. In a file with Windows line endings (CRLF), it also matches text written with plain line endings and keeps the file's line endings

`bash` keeps only the first 20,000 bytes of output and discards the rest as it arrives. It returns once the command exits, even if a background process it started keeps running. If a command times out, `bash` returns the output so far, followed by the timeout notice.

Consecutive `read` calls show as one line with the paths separated by commas, for example `read src/main.rs (2), README.md`. A number in brackets shows how many times a file was read. A failed read starts a new line. In print mode, the line is printed when the next tool, text or notice arrives.

## Development

Before each commit, run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`.

## Licence

MIT. See [LICENSE](LICENSE).
