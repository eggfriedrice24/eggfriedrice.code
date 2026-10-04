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
  `cargo test` and `just test` never invoke Zig. Until `default-members` in the root
  `Cargo.toml` is switched on (it may name only crates that exist), a bare
  `cargo build` or `cargo test` at the root does build this crate and needs Zig; the
  `test`, `lint`, `clippy` and `doc` recipes and the CI jobs name their packages or
  exclude it, so they stay Zig-free.
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
| `LIBGHOSTTY_VT_SYS_OPTIMIZE` | `Debug`, `ReleaseSafe`, `ReleaseFast` or `ReleaseSmall`; unset, it follows Cargo: `Debug` whenever Cargo sets `DEBUG=true` (the dev and test profiles), `ReleaseSmall` for `opt-level` `s` or `z`, `ReleaseFast` otherwise |

The sys crate's `pkg-config` feature skips the Zig build when an installed library is
found. Release builds set `LIBGHOSTTY_VT_SYS_CPU=baseline` so the binaries run on older
machines; `baseline` is also the default.

`default-features = false` drops the `kitty-graphics` feature. At this rev that feature
only gates the Rust wrapper of the kitty graphics API (`libghostty_vt::kitty::graphics`);
the sys crate's `build.rs` never reads it, so the Zig build is the same either way.

## Build cost

Measured on 2026-10-04 on the development machine (32 hardware threads), building
`efr-screen-ghostty` alone into an empty target directory:

| Build | Time | Note |
|---|---|---|
| dev (`cargo build -p efr-screen-ghostty`) | 33 s | Zig `Debug`; the same with an empty Zig global cache |
| release (`cargo build --release -p efr-screen-ghostty`) | 63 s | Zig `ReleaseFast`, empty Zig global cache |

Both include the partial clone of ghostty. Per target directory, the build script
keeps the clone (about 180 MB), Zig's local cache (about 350 MB) and the installed
library (about 105 MB) under `target/<profile>/build/libghostty-vt-sys-*/out/`, so
every worktree with its own target directory clones and builds again. Zig's packages
(about 100 MB) go to Zig's global cache, `~/.cache/zig` unless `ZIG_GLOBAL_CACHE_DIR`
says otherwise; `GHOSTTY_ZIG_SYSTEM_DIR` replaces that download.

The dev build's `Debug` library is slow: it takes in roughly 120 KB of plain text per
second, which is fine for the tests and too slow for a long session. Run `efrd` with
the ghostty backend from a release build, or set `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe`
for a faster dev build.

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

`just bump-ghostty <rev>` takes a full 40-character commit. It reads the new
`GHOSTTY_COMMIT` from that rev's `build.rs` and the Zig version from that commit's
`build.zig.zon` before it writes anything, then does step 1, rewrites the rev and the
ghostty commit (full and short) in `Cargo.toml` and this file, moves `Cargo.lock` to
the new rev, and prints the Zig version next to the local `zig version`. The Zig
version and the dates in this file and the Zig version in `ci.yml` are left to the
person bumping. Then run `just test-ghostty`.
libghostty-vt is pre-1.0 with no C API compatibility promise, so a bump can need code
changes, and they must stay inside `efr-screen-ghostty`.

## Next target

A libghostty-rs rev that wraps ghostty `7bb45ba34eed168dae5de24b088aa4602fbee75c`
(2026-09-30, "libghostty: add semantic prompt and reset effects"). No such rev exists
on 2026-10-03. When it lands, the effect is wired in
`efr-screen-ghostty/src/semantic_prompt.rs` only as a cross-check against the
backend-independent OSC 133 scanner (`docs/adr/0002-shell-marks-in-efr-screen.md`).
