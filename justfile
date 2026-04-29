set working-directory := "codex-rs"
set positional-arguments

rust_min_stack := "8388608" # 8 MiB

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

# Start `codex exec-server` and run codex-tui.
[no-cd]
tui-with-exec-server *args:
    {{ justfile_directory() }}/scripts/run_tui_with_exec_server.sh "$@"

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
    RUST_MIN_STACK={{ rust_min_stack }} cargo nextest run --no-fail-fast

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
    {{ justfile_directory() }}/scripts/check-module-bazel-lock.sh

bazel-test:
    bazel test --test_tag_filters=-argument-comment-lint //... --keep_going

[no-cd]
bazel-clippy:
    bazel_targets="$({{ justfile_directory() }}/scripts/list-bazel-clippy-targets.sh)" && bazel build --config=clippy -- ${bazel_targets}

[no-cd]
bazel-argument-comment-lint:
    bazel build --config=argument-comment-lint -- $({{ justfile_directory() }}/tools/argument-comment-lint/list-bazel-targets.sh)

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
#
# Each perf build prunes `./target` back down to `./target/release/codex` so a
# PATH entry or symlink targeting that binary keeps working without retaining
# the full build tree.
perf-build-local:
    #!/usr/bin/env bash
    set -euo pipefail
    export CARGO_PROFILE_RELEASE_PANIC=abort
    EXTRA_FLAGS="${CODEX_PERF_EXTRA_FLAGS:-}"
    export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=native${EXTRA_FLAGS:+ $EXTRA_FLAGS}"
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        cargo build -p codex-cli --release --locked --features "$CODEX_PERF_FEATURES"
    else
        cargo build -p codex-cli --release --locked
    fi
    ./scripts/prune_perf_build_target.sh ./target/release/codex

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
    export CARGO_PROFILE_RELEASE_PANIC=abort
    PGO_DIR="${CODEX_PGO_DIR:-${TMPDIR:-/tmp}/codex-pgo}"
    rm -rf "$PGO_DIR"
    mkdir -p "$PGO_DIR"
    LLVM_PROFDATA="$(command -v llvm-profdata || true)"
    if [ -z "$LLVM_PROFDATA" ]; then
        RUST_HOST="$(rustc -vV | sed -n 's/^host: //p')"
        RUST_LLVM_PROFDATA="$(rustc --print sysroot)/lib/rustlib/$RUST_HOST/bin/llvm-profdata"
        if [ -x "$RUST_LLVM_PROFDATA" ]; then
            LLVM_PROFDATA="$RUST_LLVM_PROFDATA"
        fi
    fi
    if [ -z "$LLVM_PROFDATA" ] && command -v xcrun >/dev/null 2>&1; then
        LLVM_PROFDATA="$(xcrun --find llvm-profdata 2>/dev/null || true)"
    fi
    if [ -z "$LLVM_PROFDATA" ]; then
        echo "llvm-profdata not found; install llvm-tools-preview or add llvm-profdata to PATH" >&2
        exit 127
    fi
    EXTRA_FLAGS="${CODEX_PERF_EXTRA_FLAGS:-}"
    COMMON_RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=native${EXTRA_FLAGS:+ $EXTRA_FLAGS}"
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-generate=$PGO_DIR" cargo build -p codex-cli --release --locked --features "$CODEX_PERF_FEATURES"
    else
        RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-generate=$PGO_DIR" cargo build -p codex-cli --release --locked
    fi
    ./target/release/codex --help >/dev/null
    ./target/release/codex exec --help >/dev/null
    ./target/release/codex mcp --help >/dev/null
    if [ -n "${CODEX_PGO_TRAIN:-}" ]; then sh -lc "$CODEX_PGO_TRAIN"; fi
    "$LLVM_PROFDATA" merge -output="$PGO_DIR/merged.profdata" "$PGO_DIR"
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-use=$PGO_DIR/merged.profdata -C llvm-args=-pgo-warn-missing-function" cargo build -p codex-cli --release --locked --features "$CODEX_PERF_FEATURES"
    else
        RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-use=$PGO_DIR/merged.profdata -C llvm-args=-pgo-warn-missing-function" cargo build -p codex-cli --release --locked
    fi
    ./scripts/prune_perf_build_target.sh ./target/release/codex

# Build a reproducible local binary tuned specifically for Apple M3 machines.
# This is functionally similar to `perf-build-local` on this Mac, but pins the
# CPU target instead of relying on `native`.
perf-build-m3:
    #!/usr/bin/env bash
    set -euo pipefail
    export CARGO_PROFILE_RELEASE_PANIC=abort
    EXTRA_FLAGS="${CODEX_PERF_EXTRA_FLAGS:-}"
    export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=apple-m3${EXTRA_FLAGS:+ $EXTRA_FLAGS}"
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        cargo build -p codex-cli --release --locked --features "$CODEX_PERF_FEATURES"
    else
        cargo build -p codex-cli --release --locked
    fi
    ./scripts/prune_perf_build_target.sh ./target/release/codex

# Build an Apple M3-tuned codex binary with faster release rebuild settings for
# local iteration. This writes the same `./target/release/codex` path as
# `perf-build-m3`, but trades some final optimization for faster compile/link.
perf-build-m3-fast:
    #!/usr/bin/env bash
    set -euo pipefail
    export CARGO_PROFILE_RELEASE_PANIC=abort
    export CARGO_PROFILE_RELEASE_LTO=thin
    export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
    EXTRA_FLAGS="${CODEX_PERF_EXTRA_FLAGS:-}"
    export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=apple-m3${EXTRA_FLAGS:+ $EXTRA_FLAGS}"
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        cargo build -p codex-cli --release --locked --features "$CODEX_PERF_FEATURES"
    else
        cargo build -p codex-cli --release --locked
    fi
    ./scripts/prune_perf_build_target.sh ./target/release/codex

# Build an Apple M3-tuned codex binary using profile-guided optimization.
perf-build-m3-pgo:
    #!/usr/bin/env bash
    set -euo pipefail
    export CARGO_PROFILE_RELEASE_PANIC=abort
    PGO_DIR="${CODEX_PGO_DIR:-${TMPDIR:-/tmp}/codex-pgo-m3}"
    rm -rf "$PGO_DIR"
    mkdir -p "$PGO_DIR"
    LLVM_PROFDATA="$(command -v llvm-profdata || true)"
    if [ -z "$LLVM_PROFDATA" ]; then
        RUST_HOST="$(rustc -vV | sed -n 's/^host: //p')"
        RUST_LLVM_PROFDATA="$(rustc --print sysroot)/lib/rustlib/$RUST_HOST/bin/llvm-profdata"
        if [ -x "$RUST_LLVM_PROFDATA" ]; then
            LLVM_PROFDATA="$RUST_LLVM_PROFDATA"
        fi
    fi
    if [ -z "$LLVM_PROFDATA" ] && command -v xcrun >/dev/null 2>&1; then
        LLVM_PROFDATA="$(xcrun --find llvm-profdata 2>/dev/null || true)"
    fi
    if [ -z "$LLVM_PROFDATA" ]; then
        echo "llvm-profdata not found; install llvm-tools-preview or add llvm-profdata to PATH" >&2
        exit 127
    fi
    EXTRA_FLAGS="${CODEX_PERF_EXTRA_FLAGS:-}"
    COMMON_RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C target-cpu=apple-m3${EXTRA_FLAGS:+ $EXTRA_FLAGS}"
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-generate=$PGO_DIR" cargo build -p codex-cli --release --locked --features "$CODEX_PERF_FEATURES"
    else
        RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-generate=$PGO_DIR" cargo build -p codex-cli --release --locked
    fi
    ./target/release/codex --help >/dev/null
    ./target/release/codex exec --help >/dev/null
    ./target/release/codex mcp --help >/dev/null
    if [ -n "${CODEX_PGO_TRAIN:-}" ]; then sh -lc "$CODEX_PGO_TRAIN"; fi
    "$LLVM_PROFDATA" merge -output="$PGO_DIR/merged.profdata" "$PGO_DIR"
    if [ -n "${CODEX_PERF_FEATURES:-}" ]; then
        RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-use=$PGO_DIR/merged.profdata -C llvm-args=-pgo-warn-missing-function" cargo build -p codex-cli --release --locked --features "$CODEX_PERF_FEATURES"
    else
        RUSTFLAGS="$COMMON_RUSTFLAGS -C profile-use=$PGO_DIR/merged.profdata -C llvm-args=-pgo-warn-missing-function" cargo build -p codex-cli --release --locked
    fi
    ./scripts/prune_perf_build_target.sh ./target/release/codex

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
    cargo run --manifest-path {{ justfile_directory() }}/codex-rs/Cargo.toml -p codex-hooks --bin write_hooks_schema_fixtures

# Run the argument-comment Dylint checks across codex-rs.
[no-cd]
argument-comment-lint *args:
    if [ "$#" -eq 0 ]; then \
      bazel build --config=argument-comment-lint -- $({{ justfile_directory() }}/tools/argument-comment-lint/list-bazel-targets.sh); \
    else \
      {{ justfile_directory() }}/tools/argument-comment-lint/run-prebuilt-linter.py "$@"; \
    fi

[no-cd]
argument-comment-lint-from-source *args:
    {{ justfile_directory() }}/tools/argument-comment-lint/run.py "$@"

# Tail logs from the state SQLite database
log *args:
    if [ "${1:-}" = "--" ]; then shift; fi; cargo run -p codex-state --bin logs_client -- "$@"
