#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
cargo test -p codex-core --test all \
  hooks_operator_smoke::operator_smoke_pack_covers_supported_command_hooks \
  -- --nocapture
