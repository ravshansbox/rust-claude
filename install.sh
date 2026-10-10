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
cargo install --locked --force --path "$folder/rust-claude"
