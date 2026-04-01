set working-directory := "codex-rs"
set positional-arguments

# Display help
help:
    just -l

# `codex`
alias c := codex
codex *args:
    cargo run --bin codex -- "$@"

# `codex exec`
exec *args:
    cargo run --bin codex -- exec "$@"

# Start codex-exec-server and run codex-tui.
[no-cd]
tui-with-exec-server *args:
    ./scripts/run_tui_with_exec_server.sh "$@"

# Run the CLI version of the file-search crate.
file-search *args:
    cargo run --bin codex-file-search -- "$@"

# Build the CLI and run the app-server test client
app-server-test-client *args:
    cargo build -p codex-cli
    cargo run -p codex-app-server-test-client -- --codex-bin ./target/debug/codex "$@"

# format code
fmt:
    cargo fmt -- --config imports_granularity=Item 2>/dev/null

fix *args:
    cargo clippy --fix --tests --allow-dirty "$@"

clippy *args:
    cargo clippy --tests "$@"

install:
    rustup show active-toolchain
    cargo fetch

# Run `cargo nextest` since it's faster than `cargo test`, though including
# --no-fail-fast is important to ensure all tests are run.
#
# Run `cargo install cargo-nextest` if you don't have it installed.
# Prefer this for routine local runs. Workspace crate features are banned, so
# there should be no need to add `--all-features`.
test:
    cargo nextest run --no-fail-fast

# Build and run Codex from source using Bazel.
# Note we have to use the combination of `[no-cd]` and `--run_under="cd $PWD &&"`
# to ensure that Bazel runs the command in the current working directory.
[no-cd]
bazel-codex *args:
    bazel run //codex-rs/cli:codex --run_under="cd $PWD &&" -- "$@"

[no-cd]
bazel-lock-update:
    bazel mod deps --lockfile_mode=update

[no-cd]
bazel-lock-check:
    ./scripts/check-module-bazel-lock.sh

bazel-test:
    bazel test --test_tag_filters=-argument-comment-lint //... --keep_going

bazel-clippy:
    bazel build --config=clippy -- //codex-rs/... -//codex-rs/v8-poc:all

[no-cd]
bazel-argument-comment-lint:
    bazel build --config=argument-comment-lint -- $(./tools/argument-comment-lint/list-bazel-targets.sh)

bazel-remote-test:
    bazel test --test_tag_filters=-argument-comment-lint //... --config=remote --platforms=//:rbe --keep_going

build-for-release:
    bazel build //codex-rs/cli:release_binaries --config=remote

# Build a machine-local codex binary with reasonable runtime-focused tuning on
# top of the existing release profile. This keeps the build to a single pass
# and intentionally skips PGO.
#
# Intended for homogeneous local boxes or clusters where `target-cpu=native`
# is acceptable.
#
# Optional environment variables:
#   CODEX_PERF_EXTRA_FLAGS='...' append extra rustc flags
#   CODEX_PERF_FEATURES='...'    pass cargo features to codex-cli
perf-build-local:
    #!/usr/bin/env bash
    set -euo pipefail
    FEATURE_ARGS=()
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        FEATURE_ARGS+=(--features "$CODEX_PERF_FEATURES")
    fi
    export CARGO_PROFILE_RELEASE_PANIC=abort
    EXTRA_FLAGS="${CODEX_PERF_EXTRA_FLAGS:-}"
    export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=native${EXTRA_FLAGS:+ $EXTRA_FLAGS}"
    cargo build -p codex-cli --release --locked "${FEATURE_ARGS[@]}"

# Build a machine-local codex binary using profile-guided optimization on top
# of the same local tuning as `perf-build-local`.
#
# Optional environment variables:
#   CODEX_PGO_DIR='...'         override the temporary profile data directory
#   CODEX_PGO_TRAIN='...'       extra representative training commands to run
#   CODEX_PERF_EXTRA_FLAGS='...' append extra rustc flags
#   CODEX_PERF_FEATURES='...'   pass cargo features to codex-cli
perf-build-local-pgo:
    #!/usr/bin/env bash
    set -euo pipefail
    FEATURE_ARGS=()
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        FEATURE_ARGS+=(--features "$CODEX_PERF_FEATURES")
    fi
    export CARGO_PROFILE_RELEASE_PANIC=abort
    PGO_DIR="${CODEX_PGO_DIR:-${TMPDIR:-/tmp}/codex-pgo}"
    rm -rf "$PGO_DIR"
    mkdir -p "$PGO_DIR"
    LLVM_PROFDATA="$(command -v llvm-profdata || xcrun --find llvm-profdata)"
    EXTRA_FLAGS="${CODEX_PERF_EXTRA_FLAGS:-}"
    COMMON_RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=native${EXTRA_FLAGS:+ $EXTRA_FLAGS}"
    RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-generate=$PGO_DIR" \\
        cargo build -p codex-cli --release --locked "${FEATURE_ARGS[@]}"
    ./target/release/codex --help >/dev/null
    ./target/release/codex exec --help >/dev/null
    ./target/release/codex mcp --help >/dev/null
    if [ -n "${CODEX_PGO_TRAIN:-}" ]; then sh -lc "$CODEX_PGO_TRAIN"; fi
    "$LLVM_PROFDATA" merge -output="$PGO_DIR/merged.profdata" "$PGO_DIR"
    RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-use=$PGO_DIR/merged.profdata -C llvm-args=-pgo-warn-missing-function" \\
        cargo build -p codex-cli --release --locked "${FEATURE_ARGS[@]}"

# Build a reproducible local binary tuned specifically for Apple M3 machines.
# This is functionally similar to `perf-build-local` on this Mac, but pins the
# CPU target instead of relying on `native`.
perf-build-m3:
    #!/usr/bin/env bash
    set -euo pipefail
    FEATURE_ARGS=()
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        FEATURE_ARGS+=(--features "$CODEX_PERF_FEATURES")
    fi
    export CARGO_PROFILE_RELEASE_PANIC=abort
    EXTRA_FLAGS="${CODEX_PERF_EXTRA_FLAGS:-}"
    export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=apple-m3${EXTRA_FLAGS:+ $EXTRA_FLAGS}"
    cargo build -p codex-cli --release --locked "${FEATURE_ARGS[@]}"

# Build an Apple M3-tuned codex binary using profile-guided optimization.
perf-build-m3-pgo:
    #!/usr/bin/env bash
    set -euo pipefail
    FEATURE_ARGS=()
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        FEATURE_ARGS+=(--features "$CODEX_PERF_FEATURES")
    fi
    export CARGO_PROFILE_RELEASE_PANIC=abort
    PGO_DIR="${CODEX_PGO_DIR:-${TMPDIR:-/tmp}/codex-pgo-m3}"
    rm -rf "$PGO_DIR"
    mkdir -p "$PGO_DIR"
    LLVM_PROFDATA="$(command -v llvm-profdata || xcrun --find llvm-profdata)"
    EXTRA_FLAGS="${CODEX_PERF_EXTRA_FLAGS:-}"
    COMMON_RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=apple-m3${EXTRA_FLAGS:+ $EXTRA_FLAGS}"
    RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-generate=$PGO_DIR" \\
        cargo build -p codex-cli --release --locked "${FEATURE_ARGS[@]}"
    ./target/release/codex --help >/dev/null
    ./target/release/codex exec --help >/dev/null
    ./target/release/codex mcp --help >/dev/null
    if [ -n "${CODEX_PGO_TRAIN:-}" ]; then sh -lc "$CODEX_PGO_TRAIN"; fi
    "$LLVM_PROFDATA" merge -output="$PGO_DIR/merged.profdata" "$PGO_DIR"
    RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-use=$PGO_DIR/merged.profdata -C llvm-args=-pgo-warn-missing-function" \\
        cargo build -p codex-cli --release --locked "${FEATURE_ARGS[@]}"

# Run the MCP server
mcp-server-run *args:
    cargo run -p codex-mcp-server -- "$@"

# Regenerate the json schema for config.toml from the current config types.
write-config-schema:
    cargo run -p codex-core --bin codex-write-config-schema

# Regenerate vendored app-server protocol schema artifacts.
write-app-server-schema *args:
    cargo run -p codex-app-server-protocol --bin write_schema_fixtures -- "$@"

[no-cd]
write-hooks-schema:
    cargo run --manifest-path ./codex-rs/Cargo.toml -p codex-hooks --bin write_hooks_schema_fixtures

# Run the argument-comment Dylint checks across codex-rs.
[no-cd]
argument-comment-lint *args:
    if [ "$#" -eq 0 ]; then \
      bazel build --config=argument-comment-lint -- $(./tools/argument-comment-lint/list-bazel-targets.sh); \
    else \
      ./tools/argument-comment-lint/run-prebuilt-linter.py "$@"; \
    fi

[no-cd]
argument-comment-lint-from-source *args:
    ./tools/argument-comment-lint/run.py "$@"

# Tail logs from the state SQLite database
log *args:
    if [ "${1:-}" = "--" ]; then shift; fi; cargo run -p codex-state --bin logs_client -- "$@"
