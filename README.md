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

Edit tool calls show a diff, and write tool calls show the first 10 lines of the new file. Both use syntax highlighting based on the file extension. Removed lines have a red background and added lines have a green background.

## Commands

| Command | Action |
| --- | --- |
| `/new` | Start a new session |
| `/resume` | Resume a previous session |
| `/model [id]` | Select a model |
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

Environment variables take priority over the settings file.

## Instructions

rust-claude adds `AGENTS.md` from your home folder and from the current directory to the system prompt.

The system prompt tells the model to put questions to the user in bold.

## Sessions

Sessions are saved to `~/.rust-claude/sessions/` as JSON Lines files.

## Tools

The agent can use these tools:

- `bash`: run a shell command. Its description suggests `ast-grep` for searching code structure and `rg` for searching text, but only the ones found on `PATH`
- `read`: read a file
- `write`: create or replace a file
- `edit`: replace text in a file

## Development

Running `cargo test` once installs a Git pre-commit hook with [cargo-husky](https://github.com/rhysd/cargo-husky). The hook runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`. The hook script is in `.cargo-husky/hooks/pre-commit`.

## Licence

MIT. See [LICENSE](LICENSE).
