#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$ROOT_DIR"

TARGET_NAME="${1:-$(basename "$ROOT_DIR")}"
PACKAGE_NAME="$(
  printf '%s' "$TARGET_NAME" \
    | tr '[:upper:]' '[:lower:]' \
    | perl -pe 's/[^a-z0-9_-]+/-/g; s/^-+//; s/-+$//; s/-{2,}/-/g'
)"

if [ -z "$PACKAGE_NAME" ]; then
  echo "failed to derive a valid Cargo package name from: $TARGET_NAME" >&2
  exit 1
fi

CRATE_NAME="$(printf '%s' "$PACKAGE_NAME" | tr '-' '_')"
CURRENT_PACKAGE_NAME="$(sed -n 's/^name = "\(.*\)"/\1/p' Cargo.toml | head -n1)"

if [ -z "$CURRENT_PACKAGE_NAME" ]; then
  echo "failed to resolve current package name from Cargo.toml" >&2
  exit 1
fi

CURRENT_CRATE_NAME="$(printf '%s' "$CURRENT_PACKAGE_NAME" | tr '-' '_')"

echo "initializing template"
echo "  source name: $TARGET_NAME"
echo "  package name: $PACKAGE_NAME"
echo "  crate name: $CRATE_NAME"

PACKAGE_NAME="$PACKAGE_NAME" perl -0pi -e 's/^name = "[^"]+"/name = "$ENV{PACKAGE_NAME}"/m' Cargo.toml

CURRENT_PACKAGE_NAME="$CURRENT_PACKAGE_NAME" PACKAGE_NAME="$PACKAGE_NAME" perl -0pi -e '
  s/\Q$ENV{CURRENT_PACKAGE_NAME}\E/$ENV{PACKAGE_NAME}/g
' README.md .env.example Dockerfile.artifact

CURRENT_CRATE_NAME="$CURRENT_CRATE_NAME" CRATE_NAME="$CRATE_NAME" perl -0pi -e '
  s/\b\Q$ENV{CURRENT_CRATE_NAME}\E\b/$ENV{CRATE_NAME}/g
' src/main.rs tests/integration.rs

PACKAGE_NAME="$PACKAGE_NAME" perl -0pi -e 's/^OTEL_SERVICE_NAME=.*/OTEL_SERVICE_NAME=$ENV{PACKAGE_NAME}/m' .env.example
PACKAGE_NAME="$PACKAGE_NAME" perl -0pi -e 's/^# .*/# $ENV{PACKAGE_NAME}/m' README.md
PACKAGE_NAME="$PACKAGE_NAME" perl -0pi -e 's/^ARG BIN_NAME=.*/ARG BIN_NAME=$ENV{PACKAGE_NAME}/m' Dockerfile.artifact

cargo build
cargo test

echo "template initialized successfully"
