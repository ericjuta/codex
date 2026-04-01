# Public Release Notes

This document records the current licensing and release posture for making the
repository public and for shipping public release artifacts.

For the fork-rationale document that explains why this repository diverges from
upstream `openai/codex`, see [fork-intent.md](./fork-intent.md).

Current intent: this is a publicly available source repository. Official public
Linux binary releases are not currently provided by this document.

## Scope

There are two separate compliance questions:

1. Can the source repository be public?
2. Can we distribute public binaries built from this repository?

The answer to the first is mostly a documentation and attribution question. The
answer to the second also depends on what third-party code is compiled into the
released artifacts.

## Repository Authored Code

Repository-authored code is licensed under [Apache-2.0](../LICENSE), unless a
particular file or bundled third-party subtree states otherwise.

The root [NOTICE](../NOTICE) file and [license.md](./license.md) document the
non-Apache material currently kept in-tree.

## Bundled Third-Party Material

The repository currently includes the following notable third-party materials:

- Ratatui-derived files in `codex-rs/tui/src/custom_terminal.rs` and
  `codex-rs/tui_app_server/src/custom_terminal.rs`, with inline MIT notices.
- WezTerm-derived Windows PTY files in `codex-rs/utils/pty/src/win/`, with
  inline MIT notices and local-modification notes.
- A bundled Meriyah parser asset at
  `codex-rs/core/src/tools/js_repl/meriyah.umd.min.js`, with its license kept
  at `third_party/meriyah/LICENSE`.
- A vendored bubblewrap source tree at `codex-rs/vendor/bubblewrap`, under
  `LGPL-2.0-or-later`.

## Source Repository Publication

For a public source repository, the baseline requirements are:

- Keep the root `LICENSE` file.
- Keep the root `NOTICE` file accurate when new bundled third-party material is
  added or removed.
- Preserve inline notices on copied or derived files.
- Keep bundled third-party license texts in-tree when referenced by shipped
  assets.
- Continue running `cargo deny check licenses` for the Rust workspace.

This repository currently meets that baseline more cleanly than before, but it
still requires release discipline when third-party code is updated.

## Binary Distribution

Binary distribution needs a stricter release gate than source publication.
Public binaries should ship with:

- `LICENSE`
- `NOTICE`
- any third-party license texts required by bundled or compiled components
- release notes that explain material third-party inclusions when relevant

If a release artifact includes code under obligations beyond simple attribution,
the release process must explicitly account for that component.

## Vendored Bubblewrap

This is the main component that needs product and legal clarity before broad
public Linux binary distribution.

Current state:

- `codex-rs/linux-sandbox/build.rs` compiles vendored bubblewrap C sources on
  Linux targets.
- `codex-rs/linux-sandbox/src/vendored_bwrap.rs` exposes that compiled entry
  point for runtime use.
- `codex-rs/linux-sandbox/README.md` documents that the helper prefers system
  `/usr/bin/bwrap`, but falls back to the vendored build path when needed.

That means vendored bubblewrap is not just present in source form; it can also
be part of Linux builds and therefore affects binary-distribution compliance.

## Recommendation

Default recommendation: do not ship public Linux release binaries that rely on
the vendored bubblewrap fallback until that lane has an explicit legal and
release-process owner.

Preferred short-term approach:

- Make public Linux release builds rely on system `bwrap`, or otherwise disable
  the vendored fallback in distributed binaries.
- Keep the vendored bubblewrap tree in source if it is still useful for local
  development, CI, or non-public builds.
- Revisit vendored-bubblewrap distribution only with a dedicated compliance
  review.

If the project later decides to ship vendored bubblewrap in public binaries, the
release process should be updated deliberately rather than relying on the source
repository notices alone.

## Working Rule

Until a separate decision is recorded, treat these as the default release rules:

- Public source repo: allowed with current notices and license files kept up to
  date.
- Public Linux binaries using vendored bubblewrap: not allowed by default.
- Public Linux binaries using system bubblewrap only: preferred interim path,
  subject to normal release review.
- No official public Linux release build pipeline is assumed by this document.
