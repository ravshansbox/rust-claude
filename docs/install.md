# Install

You need [Rust](https://rustup.rs), `curl` and `unzip`.

```sh
curl -fsSL https://raw.githubusercontent.com/ravshansbox/rust-coding-agent/main/install.sh | sh
```

[install.sh](../install.sh) downloads the source of the latest [release](https://github.com/ravshansbox/rust-coding-agent/releases) into a new folder in `~/.rust-coding-agent`, and deletes it afterwards. It does not use a shared temporary folder such as `/tmp`, where another user could add a `.cargo/config.toml` that cargo would read. If [minisign](https://jedisct1.github.io/minisign/) is on `PATH`, it checks the release signature and stops with an error if it is not valid. Without minisign, it shows a warning and goes on without the check. It then builds it with `cargo install --locked --force` in `~/.rust-coding-agent/build`, the same folder that updates use, and puts `rust-coding-agent` in `~/.cargo/bin`, with `rca` as a shorter name for it. If `rca` is already there, it is left alone. If `~/.rust-coding-agent` does not exist yet but `~/.rust-claude` from before the rename does, it moves that folder to `~/.rust-coding-agent`. Otherwise it creates `~/.rust-coding-agent` so that only you can open it. It stops with an error if `cargo` is not on `PATH`. `--force` makes cargo rebuild even if this version is already installed, so the same command also upgrades or reinstalls.

**`rca`:** every time it starts, rust-coding-agent adds `rca` as a link to itself in its own folder if nothing there has that name, so other ways of installing, such as `cargo install`, also get `rca` after the first start. A deleted `rca` comes back on the next start. Debug builds, such as from `cargo run`, do not add it. On Windows there is no `rca`.

**Upgrade or reinstall:** run the same command again.

**Updates:** when the interface starts, rust-coding-agent checks the [latest release](https://github.com/ravshansbox/rust-coding-agent/releases/latest) in the background, every time. If it is newer, rust-coding-agent adds each step to the version line while it builds and installs it with `cargo install` in the background, adding the `rca` link if it is missing, so you can keep working. When a step finishes, it shows how long the step took, for example `rust-coding-agent v0.1.0 · update v0.2.0 available · downloaded in 3s · built in 2m 5s · installed, restart rust-coding-agent to use it`. Each release is signed with [minisign](https://jedisct1.github.io/minisign/), and rust-coding-agent builds it only if `rust-coding-agent.zip.minisig` shows that the release key signed it. Otherwise it shows an error, such as `the signature of rust-coding-agent.zip is not valid`, and installs nothing. The signature's trusted comment must also name the version being installed, for example `rust-coding-agent v0.2.0`, so an older signed release cannot be passed off as a newer one. Otherwise it shows an error such as `rust-coding-agent.zip is signed as rust-coding-agent v0.2.0, not v0.3.0`. The public key is `RWSa19cgsH6sJ8TomZ0rN68H2QSZmUNhCpxJtnW6IQW/gn6XDhilg5A8`. If `cargo` is not on `PATH`, it shows a warning instead. A failed step shows how long it ran and the first error, for example `build failed after 1m 10s: error: could not compile`. It downloads and unpacks each release in `~/.rust-coding-agent/updates` (or `updates` in the `--config-dir` folder), not in a shared temporary folder, and deletes it afterwards. Builds go in `~/.rust-coding-agent/build` (or `build` in the `--config-dir` folder), so later updates reuse the dependencies that did not change and build faster. When `rustc -vV` reports a different compiler, rust-coding-agent empties that folder first, because cargo cannot reuse the old files. To free the space, delete the folder. A failed check, for example when offline, shows nothing. Debug builds, such as from `cargo run`, and builds from `main` (`v0.0.0`) do not check. To turn the check off, set `check_for_updates` to `false` in `settings.json`.

**Uninstall:**

```sh
cargo uninstall rust-coding-agent
rm ~/.cargo/bin/rca
rm -r ~/.rust-coding-agent  # optional: sign-in, settings, sessions, prompt history, skills and MCP config
```
