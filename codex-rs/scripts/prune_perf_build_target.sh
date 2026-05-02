#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <binary_path>" >&2
    exit 64
fi

binary_path="$1"
binary_dir="$(dirname -- "$binary_path")"
binary_base="$(basename -- "$binary_path")"
target_dir="$(dirname -- "$binary_dir")"
binary_stash_dir="$(mktemp -d)"
binary_stash="$binary_stash_dir/$binary_base"

cp --preserve=mode,timestamps "$binary_path" "$binary_stash"
rm -rf -- "$target_dir"
mkdir -p -- "$binary_dir"
cp --preserve=mode,timestamps "$binary_stash" "$binary_path"
rm -rf -- "$binary_stash_dir"
