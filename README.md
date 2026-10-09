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

Edit tool calls show a diff, and write tool calls show the first 10 lines of the new file. Both use syntax highlighting based on the file extension. Removed lines have a red background and added lines have a green background. Leading spaces are removed from each line to save room.

## Commands

| Command | Action |
| --- | --- |
| `/new` | Start a new session |
| `/resume` | Resume a previous session |
| `/model [id]` | Select a model. An id must be in the list of available models |
| `/thinking [level]` | Select a thinking level |
| `/quit` | Quit |

## Keys

| Key | Action |
| --- | --- |
| Enter | Send prompt |
| Esc | Cancel the running prompt, or quit when idle |
| Ctrl+C | Clear input, or quit when input is empty |
| Ctrl+D | Quit when input is empty |
| Shift+Tab | Cycle thinking level |
| Up / Down | Browse prompt history, or scroll |
| Page Up / Page Down | Scroll by a page |
| Home / End | Scroll to top or bottom |
| Left / Right | Move cursor |
| Alt+Left / Alt+B, Alt+Right / Alt+F | Move by word |
| Ctrl+A / Ctrl+E | Jump to start or end of input |
| Alt+Backspace / Ctrl+W | Delete previous word |
| Tab | Complete a command |

While a reply streams, the view follows new text only when it is scrolled to the bottom. If you scroll up, the view stays where it is.

## Status line

The status line shows token use for the session: input (↑), output (↓), cache reads (R), cache writes (W), the cache hit rate (CH) and the average output speed in tokens per second (tps). A `·` separates context use, token counts and speed.

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

## Sessions

Sessions are saved to `~/.rust-claude/sessions/` as JSON Lines files.

Quitting while a prompt runs cancels it and saves the session first.

A cancelled prompt stays in the session, so the model sees it in the next request. A resumed session shows messages, tool calls and failed tool calls as they appeared live.

## Tools

The agent can use these tools:

- `bash`: run a shell command. Its description suggests `ast-grep` for searching code structure and `rg` for searching text, but only the ones found on `PATH`
- `read`: read a file
- `write`: create or replace a file
- `edit`: replace text in a file

Consecutive `read` calls show as one line with the paths separated by commas, for example `read src/main.rs (2), README.md`. A number in brackets shows how many times a file was read. A failed read starts a new line. In print mode, the line is printed when the next tool, text or notice arrives.

## Development

Running `cargo test` once installs a Git pre-commit hook with [cargo-husky](https://github.com/rhysd/cargo-husky). The hook runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`. The hook script is in `.cargo-husky/hooks/pre-commit`.

## Licence

MIT. See [LICENSE](LICENSE).
