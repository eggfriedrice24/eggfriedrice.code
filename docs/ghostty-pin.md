# The libghostty-vt pin

`efr-screen-ghostty` renders terminal output with libghostty-vt. The pin is a triple,
and all three parts move together.

| Component | Pinned value | Why |
|---|---|---|
| libghostty-rs | git rev `8953a740bc378cec3e07e1f6ca949f0595eab19b` (master, 2026-09-28; workspace version 0.2.1) | master has `snapshot.rs` (the GHOSTSNP record stream behind `Terminal::encode_snapshot` and `Decoder`) and `io.rs`; attach snapshots need them. The constructor is `Terminal::new(cols, rows)`. |
| ghostty | `22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018` (2026-08-06) | the `GHOSTTY_COMMIT` that this rev's `crates/libghostty-vt-sys/build.rs` clones and builds. It follows from the rev; it is not a separate choice. |
| Zig | 0.16.0 exactly | ghostty 22d13172's `build.zig.zon` sets `minimum_zig_version = "0.16.0"`, and libghostty-rs pins 0.16.0. A build of 22d13172 under Zig 0.17.0 (released 2026-10-01) is unverified, so the pin is exact. |

In `Cargo.toml`:

```toml
libghostty-vt = { git = "https://github.com/uzaaft/libghostty-rs", rev = "8953a740bc378cec3e07e1f6ca949f0595eab19b", default-features = false }
```

`deny.toml` allows this one git source. CI installs Zig with `mlugg/setup-zig` at
`version: 0.16.0` in the `ghostty` job. This file, `Cargo.toml` and `ci.yml` are the
only places the triple is written.

## Isolation

- Only `efr-screen-ghostty` may depend on `libghostty-vt` (and through it on
  `libghostty-vt-sys`, which runs `zig build`). `cargo xtask deps` and the CI
  `cargo tree` checks fail on any other path.
- `efr-screen-ghostty` is not a default member, and `efr-daemon` depends on it only
  through the `screen-ghostty` feature, which is off by default. `cargo build`,
  `cargo test` and `just test` never invoke Zig.
- `just test-ghostty`, `just build-release` and the release build turn the feature on.
- libghostty-vt types are `!Send + !Sync`; see the threading model in
  `ARCHITECTURE.md` and `docs/adr/0003-screen-thread-per-actor.md`.

## Why not crates.io 0.2.2

crates.io has `libghostty-vt` 0.2.2 and `libghostty-vt-sys` 0.2.2 (published
2026-09-28 from the `release/0.2.x` branch, 48 commits behind master). It is rejected:

- it pins ghostty `a887df42` (2026-07-11), which needs Zig 0.15.2, so it needs a
  different Zig than the git pin;
- it has no `snapshot.rs`, so it cannot serve attach snapshots;
- its constructor is `Terminal::new(opts: Options)`, so moving to master later would be
  an API change.

Vendoring the ghostty source (herdr's approach, with a patch ledger) is also rejected
for now. It is the right move only when a local patch is needed.

## Build knobs of the sys crate

| Variable | Effect |
|---|---|
| `GHOSTTY_SOURCE_DIR` | use this ghostty checkout instead of cloning; it must contain `build.zig`, and it wins over everything else |
| `GHOSTTY_ZIG_SYSTEM_DIR` | passed to `zig build --system` for builds without network access |
| `LIBGHOSTTY_VT_SYS_CPU` | target CPU; default `baseline`, `native` for a build that only runs on this machine |
| `LIBGHOSTTY_VT_SYS_OPTIMIZE` | `Debug`, `ReleaseSafe`, `ReleaseFast` or `ReleaseSmall` |

The sys crate's `pkg-config` feature skips the Zig build when an installed library is
found. Release builds set `LIBGHOSTTY_VT_SYS_CPU=baseline` so the binaries run on older
machines.

The sys crate clones from a hard-coded GitHub URL. If that mirror stops updating, set
`GHOSTTY_SOURCE_DIR` to a checkout of the pinned commit, or keep a mirror under the
project's control.

## Bump procedure

A bump is one commit that changes:

1. the `rev` in the workspace `Cargo.toml`;
2. this file: the libghostty-rs rev, the ghostty commit its `build.rs` names, and the
   Zig version from that commit's `build.zig.zon`;
3. the Zig version in `.github/workflows/ci.yml` (and the release setup step);
4. the conformance fixtures under `crates/efr-screen/fixtures/`, if rendering changed.

`just bump-ghostty <rev>` does step 1, rewrites the rev in this file and prints the
ghostty commit and Zig version of the new pin. Then run `just test-ghostty`.
libghostty-vt is pre-1.0 with no C API compatibility promise, so a bump can need code
changes, and they must stay inside `efr-screen-ghostty`.

## Next target

A libghostty-rs rev that wraps ghostty `7bb45ba34eed168dae5de24b088aa4602fbee75c`
(2026-09-30, "libghostty: add semantic prompt and reset effects"). No such rev exists
on 2026-10-03. When it lands, the effect is wired in
`efr-screen-ghostty/src/semantic_prompt.rs` only as a cross-check against the
backend-independent OSC 133 scanner (`docs/adr/0002-shell-marks-in-efr-screen.md`).
