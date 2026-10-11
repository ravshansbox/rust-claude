#!/bin/sh
set -eu

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found on PATH, install Rust from https://rustup.rs" >&2
  exit 1
fi

folder=$(mktemp -d)
trap 'rm -rf "$folder"' EXIT

curl -fsSL -o "$folder/rust-coding-agent.zip" \
  https://github.com/ravshansbox/rust-coding-agent/releases/latest/download/rust-coding-agent.zip
unzip -q "$folder/rust-coding-agent.zip" -d "$folder"
if [ ! -e "$HOME/.rust-coding-agent" ] && [ -d "$HOME/.rust-claude" ]; then
  mv "$HOME/.rust-claude" "$HOME/.rust-coding-agent"
fi
build="$HOME/.rust-coding-agent/build"
(umask 077 && mkdir -p "$build")
if version=$(rustc -vV 2>/dev/null) && [ "$(cat "$build/rustc-version" 2>/dev/null)" != "$version" ]; then
  rm -rf "$build"
  (umask 077 && mkdir -p "$build")
  printf '%s\n' "$version" > "$build/rustc-version"
fi
cargo install --locked --force --target-dir "$build" --path "$folder/rust-coding-agent"
