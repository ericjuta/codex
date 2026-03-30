## License

This repository is licensed under the [Apache-2.0 License](../LICENSE). Unless
an individual file or bundled third-party directory says otherwise, that is the
license that applies to repository-authored code.

## Bundled Third-Party Material

Some files shipped in this repository remain under their upstream licenses and
keep their original notices:

- codex-rs/tui/src/custom_terminal.rs and
  codex-rs/tui_app_server/src/custom_terminal.rs are derived from Ratatui and
  retain MIT notices inline.
- codex-rs/utils/pty/src/win/ contains Windows PTY support code copied from
  WezTerm and retains MIT notices inline. A copy of the upstream license is
  available at third_party/wezterm/LICENSE.
- codex-rs/core/src/tools/js_repl/meriyah.umd.min.js bundles a Meriyah parser
  asset under the ISC license. A copy of that license is available at
  third_party/meriyah/LICENSE.
- codex-rs/vendor/bubblewrap/ vendors bubblewrap source code under
  LGPL-2.0-or-later. The full license text is at
  codex-rs/vendor/bubblewrap/COPYING.

The root [NOTICE](../NOTICE) file summarizes the bundled third-party materials
that currently require explicit attribution in this source tree.

For release and publication guidance, including the current recommendation for
vendored bubblewrap, see [fork-intent.md](./fork-intent.md).

## Package Metadata

Published Rust crates under codex-rs, the JavaScript packages under codex-cli,
and the Python and TypeScript SDK packages under sdk/ declare Apache-2.0 in
their package metadata.

## Dependency Auditing

Rust dependency licenses are checked with cargo-deny:

    cd codex-rs
    cargo deny check licenses
