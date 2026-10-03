# 0001: One crate per bounded context, checked by an allowlist

Status: accepted, 2026-10-03.

## Context

The design names several invariants: the protocol crate has no runtime, one crate owns
SQLite, one crate needs Zig, tools never decide permissions, the OpenAI provider never
sees refresh tokens, and the shell layer gets PTYs through a trait. Conventions that a
reviewer must remember erode. Three workspace shapes were judged: a boundary-first
workspace, a codex-like precedent workspace with fewer crates, and a minimal one with
about ten crates. The boundary-first shape won on boundaries and testability; it lost
points on iteration speed, because a cross-cutting change touches several manifests.

## Decision

- One crate per bounded context, dependencies pointing down through tiers 0 to 4, one
  protocol crate shared by the daemon and every client, and a composition root
  (`efr-daemon`) with one file per protocol method.
- `xtask/src/deps.rs` holds an allowlist of workspace dependencies for every crate and a
  list of forbidden edges, checked transitively on `cargo metadata`. CI repeats the
  forbidden edges with `cargo tree`.
- A module becomes a crate when it gets heavy dependencies of its own or a second binary
  needs it, and folds back when neither holds. The deps file is the only place that
  changes.

## Consequences

- Each invariant is a failing check, not a review comment.
- Leaf crates test with nothing but `efr-test-support`; the whole suite runs without Zig.
- More manifests and READMEs exist before the first feature works, and a new edge needs
  a deliberate one-line change plus a reason in the crate's README.
