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

In print mode, tool calls go to standard error. Add `--hide-tools` to hide them.

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

## Settings

| Setting | Environment variable | `~/.rust-claude/settings.json` key | Default |
| --- | --- | --- | --- |
| Model | `RUST_CLAUDE_MODEL` | `model` | `claude-opus-5-5` |
| Thinking level | `RUST_CLAUDE_THINKING` | `thinking_level` | `medium` |

Thinking levels: `low`, `medium`, `high`, `xhigh`, `max`.

Environment variables take priority over the settings file.

## Instructions

rust-claude adds `AGENTS.md` from your home folder and from the current directory to the system prompt.

## Sessions

Sessions are saved to `~/.rust-claude/sessions/` as JSON Lines files.

## Tools

The agent can use these tools:

- `bash`: run a shell command
- `read`: read a file
- `write`: create or replace a file
- `edit`: replace text in a file

## Licence

MIT. See [LICENSE](LICENSE).
