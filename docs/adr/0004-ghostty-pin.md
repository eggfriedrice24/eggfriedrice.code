# 0004: Pin libghostty-vt by git rev, not crates.io 0.2.2

Status: accepted, 2026-10-03.

## Context

libghostty-vt is pre-1.0 and has no C API compatibility promise. Two lines exist on
2026-10-03: libghostty-rs master and crates.io 0.2.2, published from a release branch 48
commits behind master. The daemon needs attach snapshots (GHOSTSNP), which only master
has. The sys crate builds ghostty with Zig, and the required Zig version follows from
the ghostty commit it clones.

## Decision

Pin the triple libghostty-rs `8953a740bc378cec3e07e1f6ca949f0595eab19b`, ghostty
`22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018` and Zig 0.16.0 exactly, recorded in
`docs/ghostty-pin.md`. Reject crates.io 0.2.2: it pins ghostty `a887df42` (Zig 0.15.2),
has no snapshot API and has a different constructor. Do not vendor the ghostty source
until a local patch is needed.

## Consequences

- `deny.toml` allows exactly one git source.
- Zig is needed only for `efr-screen-ghostty`, `just test-ghostty`, the `ghostty` CI job
  and release builds.
- A bump is one commit across `Cargo.toml`, `docs/ghostty-pin.md`, `ci.yml` and, if
  rendering changed, the conformance fixtures; code changes stay in one crate.
- The sys crate clones ghostty from GitHub at build time; `GHOSTTY_SOURCE_DIR` or a
  project mirror is the fallback if that source goes away.
