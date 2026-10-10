#!/bin/sh
set -eu

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found on PATH, install Rust from https://rustup.rs" >&2
  exit 1
fi

folder=$(mktemp -d)
trap 'rm -rf "$folder"' EXIT

curl -fsSL -o "$folder/rust-claude.zip" \
  https://github.com/ravshansbox/rust-claude/releases/latest/download/rust-claude.zip
unzip -q "$folder/rust-claude.zip" -d "$folder"
build="$HOME/.rust-claude/build"
if version=$(rustc -vV 2>/dev/null) && [ "$(cat "$build/rustc-version" 2>/dev/null)" != "$version" ]; then
  rm -rf "$build"
  mkdir -p "$build"
  printf '%s\n' "$version" > "$build/rustc-version"
fi
cargo install --locked --force --target-dir "$build" --path "$folder/rust-claude"
