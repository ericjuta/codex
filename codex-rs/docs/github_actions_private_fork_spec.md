# GitHub Actions Private Fork Spec

## Goal

Define which GitHub Actions workflows are worth keeping in a private personal
fork of this repository and which should usually be disabled or removed.

## Non-Goals

- changing upstream CI policy for the canonical repo
- designing a public release process
- replacing local development checks with hosted CI

## Workflow Inventory

Observed workflows fall into these buckets:

- product CI
- dependency and security checks
- release automation
- contributor governance
- issue and PR triage
- vendored upstream CI

Examples of likely keep candidates:

- rust-ci.yml
- bazel.yml
- cargo-deny.yml
- codex-rs/.github/workflows/cargo-audit.yml
- codespell.yml
- sdk.yml

Examples of workflows that are usually unnecessary in a private fork:

- cla.yml
- close-stale-contributor-prs.yml
- issue-deduplicator.yml
- issue-labeler.yml
- blob-size-policy.yml
- rust-release-argument-comment-lint.yml
- rust-release-prepare.yml
- rust-release-windows.yml
- rust-release-zsh.yml
- rust-release.yml
- rusty-v8-release.yml
- v8-canary.yml
- codex-rs/vendor/bubblewrap/.github/workflows/check.yml

## Desired Outcomes

1. Keep enough CI to catch regressions in code paths the fork owner actually
   uses.
2. Avoid wasting Actions minutes on public-maintainer workflows that have no
   value in a private repo.
3. Keep the retained workflow set easy to understand and maintain.

## Classification

### Keep

- rust-ci.yml when Cargo and Rust are the main development path
- codex-rs/.github/workflows/cargo-audit.yml for vulnerability visibility
- cargo-deny.yml for dependency and license policy signal

### Optional

- bazel.yml if Bazel is the real source of truth for the fork
- codespell.yml if cheap docs hygiene is still useful
- sdk.yml if the SDK packages are actively used
- ci.yml if the root JS/npm package and docs packaging flow matter to the fork

### Usually Remove Or Disable

- contributor governance workflows
- issue triage workflows
- PR governance workflows
- release and publishing workflows
- vendored upstream workflows

## Recommended Baselines

### Minimal Cargo-First Fork

- keep rust-ci.yml
- keep codex-rs/.github/workflows/cargo-audit.yml
- keep cargo-deny.yml
- optionally keep codespell.yml
- disable or remove the rest

### Bazel-First Fork

- keep bazel.yml
- keep codex-rs/.github/workflows/cargo-audit.yml
- keep cargo-deny.yml
- optionally keep rust-ci.yml and codespell.yml

## Acceptance Criteria

1. At least one real product CI lane remains enabled.
2. Security and dependency visibility remains available through cargo-audit or
   an equivalent workflow.
3. CLA, stale PR, issue triage, and release workflows no longer consume Actions
   runs unless the fork explicitly needs them.
4. The retained workflow set is documented in terms of why each workflow still
   exists.

## Recommendation

For this repository as a private personal fork, the default recommendation is
to keep rust-ci.yml, codex-rs/.github/workflows/cargo-audit.yml, and
cargo-deny.yml; optionally keep bazel.yml and codespell.yml; and disable or
remove the contributor, release, issue-triage, and vendored-upstream
workflows.
