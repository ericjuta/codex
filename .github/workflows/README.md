# Workflow Strategy

This fork keeps only the hosted CI lanes that still provide direct value for the
private Rust-first workflow.

## Retained Workflows

- `rust-ci.yml` provides the primary cross-platform Rust validation signal for
  changes in this fork.
- `cargo-deny.yml` preserves dependency and license-policy visibility.

## Removed Workflow Categories

The fork intentionally does not keep hosted workflows for:

- public-maintainer governance such as CLA or stale-PR handling
- issue and PR triage automation
- release publishing and packaging automation
- heavyweight post-merge validation lanes that are not required for this fork's
  day-to-day development path

## Rule Of Thumb

- keep workflow scope narrow and directly tied to the code paths this fork still
  develops and ships
- prefer local or manually-invoked validation for heavyweight checks that do not
  need to consume hosted CI on every branch update
