# Rust Coding Agent

A small coding agent for the terminal, written in Rust. It sends your requests to Anthropic's Claude models and can run shell commands and read, write and edit files in the current directory.

Website: <https://ravshansbox.github.io/rust-coding-agent/>

## Features

- Sign in with a Claude Pro or Max account
- Interactive terminal interface with streamed replies
- Built-in tools: `bash`, `read`, `write`, `edit`, `agent`, `stop` and `web_search`
- Subagents that run tasks at the same time or one after another
- Background commands, such as watching a CI run, whose output comes back when they finish
- Print mode (`-p`) to run one prompt and print the answer
- Four models, effort levels and thinking shown in the conversation
- Saved sessions you can resume, automatic compaction and prompt caching
- Status line with cache tokens, context use and quota
- Paste images, `@` file picker and prompt history search
- `AGENTS.md` instructions and [Agent Skills](https://agentskills.io/specification)
- MCP servers over stdio or Streamable HTTP, with OAuth sign-in
- Retries for rate limits, overload and connection errors
- Background updates, with what's new shown after each one

## Install

You need [Rust](https://rustup.rs), `curl` and `unzip`.

```sh
curl -fsSL https://raw.githubusercontent.com/ravshansbox/rust-coding-agent/main/install.sh | sh
```

[install.sh](install.sh) downloads the source of the latest [release](https://github.com/ravshansbox/rust-coding-agent/releases) into a new folder in `~/.rust-coding-agent`, and deletes it afterwards. It does not use a shared temporary folder such as `/tmp`, where another user could add a `.cargo/config.toml` that cargo would read. If [minisign](https://jedisct1.github.io/minisign/) is on `PATH`, it checks the release signature and stops with an error if it is not valid. Without minisign, it shows a warning and goes on without the check. It then builds it with `cargo install --locked --force` in `~/.rust-coding-agent/build`, the same folder that updates use, and puts `rust-coding-agent` in `~/.cargo/bin`, with `rca` as a shorter name for it. If `rca` is already there, it is left alone. If `~/.rust-coding-agent` does not exist yet but `~/.rust-claude` from before the rename does, it moves that folder to `~/.rust-coding-agent`. Otherwise it creates `~/.rust-coding-agent` so that only you can open it. It stops with an error if `cargo` is not on `PATH`. `--force` makes cargo rebuild even if this version is already installed, so the same command also upgrades or reinstalls.

**Upgrade or reinstall:** run the same command again.

**Updates:** when the interface starts, rust-coding-agent checks the [latest release](https://github.com/ravshansbox/rust-coding-agent/releases/latest) in the background, every time. If it is newer, rust-coding-agent adds each step to the version line while it builds and installs it with `cargo install` in the background, adding the `rca` link if it is missing, so you can keep working. When a step finishes, it shows how long the step took, for example `rust-coding-agent v0.1.0 · update v0.2.0 available · downloaded in 3s · built in 2m 5s · installed, restart rust-coding-agent to use it`. Each release is signed with [minisign](https://jedisct1.github.io/minisign/), and rust-coding-agent builds it only if `rust-coding-agent.zip.minisig` shows that the release key signed it. Otherwise it shows an error, such as `the signature of rust-coding-agent.zip is not valid`, and installs nothing. The signature's trusted comment must also name the version being installed, for example `rust-coding-agent v0.2.0`, so an older signed release cannot be passed off as a newer one. Otherwise it shows an error such as `rust-coding-agent.zip is signed as rust-coding-agent v0.2.0, not v0.3.0`. The public key is `RWSa19cgsH6sJ8TomZ0rN68H2QSZmUNhCpxJtnW6IQW/gn6XDhilg5A8`. If `cargo` is not on `PATH`, it shows a warning instead. A failed step shows how long it ran and the first error, for example `build failed after 1m 10s: error: could not compile`. It downloads and unpacks each release in `~/.rust-coding-agent/updates` (or `updates` in the `--config-dir` folder), not in a shared temporary folder, and deletes it afterwards. Builds go in `~/.rust-coding-agent/build` (or `build` in the `--config-dir` folder), so later updates reuse the dependencies that did not change and build faster. When `rustc -vV` reports a different compiler, rust-coding-agent empties that folder first, because cargo cannot reuse the old files. To free the space, delete the folder. A failed check, for example when offline, shows nothing. Debug builds, such as from `cargo run`, and builds from `main` (`v0.0.0`) do not check. To turn the check off, set `check_for_updates` to `false` in `settings.json`.

**What's new:** rust-coding-agent saves its version as `last_version` in `settings.json`. On the first launch after an update, it shows the [release notes](https://github.com/ravshansbox/rust-coding-agent/releases) of the new version, for example `what's new in v0.2.0:` followed by its changes. The first launch after installing shows nothing, because there is no earlier version to compare with. A failed check shows nothing. It follows the same rules as the update check, so debug builds, builds from `main`, and `check_for_updates` set to `false` turn it off.

**Uninstall:**

```sh
cargo uninstall rust-coding-agent
rm ~/.cargo/bin/rca
rm -r ~/.rust-coding-agent  # optional: sign-in, settings, sessions, prompt history, skills and MCP config
```

## Sign in

On first run, rust-coding-agent prints a sign-in link for your Claude Pro or Max account to standard error and asks for the code. Paste the whole code the page shows, including the part after `#`; rust-coding-agent checks that part against the sign-in link and stops with an error if it is missing or does not match. It saves the sign-in to `~/.rust-coding-agent/auth.json` and renews it when it expires. It renews 5 minutes early, or halfway through the token's lifetime when that is shorter than 10 minutes. A token sent without an expiry time is used until the API turns it down. Cancelling a prompt with Esc while the sign-in renews still saves the renewed sign-in. Quitting, or stopping print mode with Ctrl+C, waits up to 5 seconds for a renewal in progress to be saved. If it cannot save a new or renewed sign-in, it keeps using it until it quits. It shows the error on standard error after you sign in, and in the renewal notice after it renews. It writes a temporary file and then replaces `auth.json` with it, so other running copies never read a half-written file and a crash keeps the previous sign-in. Running copies renew one at a time, taking turns through a lock on `~/.rust-coding-agent/auth.lock`. Before renewing, each checks `auth.json` for a sign-in another running rust-coding-agent has already renewed, and uses that one, so several running copies stay signed in. If renewing fails, it checks `auth.json` again. At start, it asks you to sign in again only when the server turns the sign-in down or `auth.json` is not valid. If it cannot reach the server, or the server has a problem, it stops with that error and keeps the saved sign-in for the next run. During a session, it tries renewing again as described in [Retries](docs/configuration.md#retries). Only when the server turns the sign-in down does the prompt fail and ask you to restart rust-coding-agent to sign in again.

## Usage

Start the interactive interface:

```sh
rust-coding-agent
```

Run one prompt and print the answer:

```sh
rust-coding-agent -p "explain src/main.rs"
```

Show all options with `-h` or `--help`.

## Documentation

- [Usage](docs/usage.md): command line, commands, keys, input, display, sessions and images
- [Configuration](docs/configuration.md): settings, instructions, skills and retries
- [Tools](docs/tools.md): built-in tools, background commands, web search and subagents
- [MCP servers](docs/mcp.md): setup and sign-in
- [Architecture decision records](docs/adr)

## Development

Before each commit, run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`. Tests keep sign-in, sessions and other files in a temporary folder, not in `~/.rust-coding-agent`.

Every push to `main` runs these checks and then makes a release if needed. The checks for each push start straight away, but only one release runs at a time. The checks cache downloaded crates and build output with [actions/cache](https://github.com/actions/cache). The cache is rebuilt when `Cargo.lock` or the Rust version changes. [git-cliff](https://git-cliff.org) works out the next version from the [conventional commit](https://www.conventionalcommits.org) messages since the last tag: `fix` and `perf` bump the patch version, `feat` the minor version and a breaking change (`feat!` or `BREAKING CHANGE`) the major version. Commits of other types, such as `docs` or `chore`, do not make a release. The release adds tag `v<version>`, a `rust-coding-agent.zip` with the source and the version set in `Cargo.toml`, and release notes that list the `feat`, `fix` and `perf` commit messages since the last tag. Nothing is pushed to `main`, so `Cargo.toml` there stays at `0.0.0`.

The website is plain HTML and CSS in [site](site). Pushes to `main` that change `site` deploy it to GitHub Pages with [.github/workflows/pages.yml](.github/workflows/pages.yml).

## Licence

MIT. See [LICENSE](LICENSE). The syntax highlighting themes use colours from github-nvim-theme, also MIT. See [assets/themes/LICENSE-github-nvim-theme](assets/themes/LICENSE-github-nvim-theme).
