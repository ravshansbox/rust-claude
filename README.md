# Rust Coding Agent

A small coding agent for the terminal, written in Rust. It sends your requests to Anthropic's Claude models and can run shell commands and read, write and edit files in the current directory.

Website: <https://ravshansbox.github.io/rust-coding-agent/>

## Features

- Sign in with a Claude Pro or Max account
- Interactive terminal interface with streamed replies
- Built-in tools: `bash`, `read`, `write`, `edit`, `agent`, `bash_stop`, `web_search` and `web_fetch`
- Subagents that run tasks at the same time or one after another
- Background commands, such as watching a CI run, whose output comes back when they finish
- Print mode (`--print` or standard input) to run one prompt and print the answer
- Four models, effort levels and thinking shown in the conversation
- Saved sessions you can resume, automatic compaction and prompt caching
- Status line with tokens, context use and quota
- Paste images or let the model read image files, `@` file picker and prompt history search
- `AGENTS.md` instructions and [Agent Skills](https://agentskills.io/specification)
- MCP servers over stdio or Streamable HTTP, with OAuth sign-in
- Retries for rate limits, overload and connection errors
- Background updates

## Install

You need [Rust](https://rustup.rs), `curl` and `unzip`.

```sh
curl -fsSL https://raw.githubusercontent.com/ravshansbox/rust-coding-agent/main/install.sh | sh
```

Run the same command again to upgrade. rust-coding-agent also updates itself in the background. See [Install](docs/install.md) for how install and updates work, and how to uninstall.

## Sign in

On first run, rust-coding-agent prints a sign-in link for your Claude Pro or Max account and asks for the code. See [Sign in](docs/sign-in.md) for details.

## Usage

Start the interactive interface:

```sh
rca
```

Run one prompt and print the answer:

```sh
rca --print "explain src/main.rs"
```

Show all options with `--help`.

## Documentation

- [Install](docs/install.md): install, updates and uninstall
- [Sign in](docs/sign-in.md): sign-in and renewal
- [Usage](docs/usage.md): command line, commands, keys, input, display, sessions and images
- [Configuration](docs/configuration.md): settings, instructions, skills and retries
- [Tools](docs/tools.md): built-in tools, background commands, web search, web fetch and subagents
- [MCP servers](docs/mcp.md): setup and sign-in
- [Development](docs/development.md): checks, CI, releases and the website
- [Architecture decision records](docs/adr)

## Development

Before each commit, run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`. Write commit messages as [conventional commits](https://www.conventionalcommits.org). See [Development](docs/development.md) for CI, releases and the website.

## Licence

MIT. See [LICENSE](LICENSE). The syntax highlighting themes use colours from github-nvim-theme, also MIT. See [assets/themes/LICENSE-github-nvim-theme](assets/themes/LICENSE-github-nvim-theme).
