#!/bin/sh
set -eu

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found on PATH, install Rust from https://rustup.rs" >&2
  exit 1
fi

if [ ! -e "$HOME/.rust-coding-agent" ] && [ -d "$HOME/.rust-claude" ]; then
  mv "$HOME/.rust-claude" "$HOME/.rust-coding-agent"
fi
(umask 077 && mkdir -p "$HOME/.rust-coding-agent")
folder=$(mktemp -d "$HOME/.rust-coding-agent/install.XXXXXX")
trap 'rm -rf "$folder"' EXIT

url=https://github.com/ravshansbox/rust-coding-agent/releases/latest/download/rust-coding-agent.zip
curl -fsSL -o "$folder/rust-coding-agent.zip" "$url"
if command -v minisign >/dev/null 2>&1; then
  curl -fsSL -o "$folder/rust-coding-agent.zip.minisig" "$url.minisig"
  if ! minisign -V -q -P RWSa19cgsH6sJ8TomZ0rN68H2QSZmUNhCpxJtnW6IQW/gn6XDhilg5A8 \
    -m "$folder/rust-coding-agent.zip" -x "$folder/rust-coding-agent.zip.minisig"; then
    echo "the signature of rust-coding-agent.zip is not valid" >&2
    exit 1
  fi
else
  echo "minisign not found on PATH, installing without checking the signature" >&2
fi
unzip -q "$folder/rust-coding-agent.zip" -d "$folder"
build="$HOME/.rust-coding-agent/build"
(umask 077 && mkdir -p "$build")
if version=$(rustc -vV 2>/dev/null) && [ "$(cat "$build/rustc-version" 2>/dev/null)" != "$version" ]; then
  rm -rf "$build"
  (umask 077 && mkdir -p "$build")
  printf '%s\n' "$version" > "$build/rustc-version"
fi
cargo install --locked --force --target-dir "$build" --path "$folder/rust-coding-agent"
bin="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin"
if [ ! -e "$bin/rca" ] && [ ! -L "$bin/rca" ]; then
  mkdir -p "$bin"
  ln -s rust-coding-agent "$bin/rca"
fi
