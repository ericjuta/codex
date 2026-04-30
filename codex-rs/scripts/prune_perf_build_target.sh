#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <target-binary-path>" >&2
    exit 64
fi

BINARY_PATH="$1"

case "$BINARY_PATH" in
    ./target/* | target/*) ;;
    *)
        echo "expected a binary path under ./target, got: $BINARY_PATH" >&2
        exit 64
        ;;
esac

if [ ! -x "$BINARY_PATH" ]; then
    echo "expected executable codex binary at $BINARY_PATH" >&2
    exit 1
fi

STAGING_DIR="$(mktemp -d "${TMPDIR:-/tmp}/codex-perf-build.XXXXXX")"
trap 'rm -rf "$STAGING_DIR"' EXIT

STAGED_BINARY="$STAGING_DIR/$(basename "$BINARY_PATH")"
cp -p "$BINARY_PATH" "$STAGED_BINARY"

rm -rf ./target
mkdir -p "$(dirname "$BINARY_PATH")"
mv "$STAGED_BINARY" "$BINARY_PATH"

"$BINARY_PATH" --help >/dev/null
