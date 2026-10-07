# efr-screen-ghostty

## Purpose

The libghostty-vt screen backend: `efr_screen::Screen` over a libghostty-vt
`Terminal`, the terminal emulator inside Ghostty. It is the only crate that links
libghostty-vt, so it is the only crate whose build needs Zig.

- `terminal`: `GhosttyScreen`, `GhosttyConfig` and the factories `factory`,
  `factory_with` and `restore_factory` that `ScreenActor::spawn` runs on the screen
  thread. libghostty-vt types are neither `Send` nor `Sync`, so the screen is built
  there with `Terminal::new` (never a borrowed allocator) and never leaves. A factory
  that cannot build its terminal logs the error and panics; the actor turns that into
  `ScreenError::Closed` for every handle.
- `effects`: the effect callbacks. libghostty-vt runs them inside `vt_write`, so they
  push answers to terminal queries (DA, DSR, DECRQM, OSC 10 and 11, in-band resize
  reports), bells and title changes into `RefCell` buffers, and the screen drains
  them into the `ScreenSink` after each feed and resize.
- `snapshot`: the conversion to the wire `ScreenSnapshot` (visible rows, scrollback,
  cursor, title, alternate screen) and GHOSTSNP, libghostty-vt's own snapshot of the
  whole terminal, parser state included. `GhosttyScreen::encode_snapshot` writes it
  and `GhosttyScreen::restore` reads it back, here or in another process.
- `semantic_prompt`: the cross-check. A second `ShellMarkScanner` runs over what the
  screen is fed; the bytes go to libghostty-vt in pieces that end at each mark, and
  after each piece libghostty-vt's working directory (`Terminal::pwd`) and the cursor
  row's prompt state must agree with the mark. A disagreement is a `warn` log line;
  a feed never fails. When a libghostty-rs pin exposes ghostty's semantic prompt
  effect (ghostty 7bb45ba), it is wired here only.

The marks themselves never come from here: the actor's scanner in `efr-screen` is
the source of truth for every backend.

The pin (libghostty-rs rev, ghostty commit, Zig version) is in `docs/ghostty-pin.md`.
This crate is not a default workspace member, and `efr-daemon` reaches it only
through its `screen-ghostty` feature, so `cargo build` and `cargo test` at the root
never run Zig once `default-members` in the root `Cargo.toml` is switched on. Until
then a bare root build includes this crate; the `just` gates and the CI jobs name
their packages or exclude it.

## Tier

Tier 2.

## Allowed dependencies

`efr-screen` only; the wire types come through its re-exports.
`xtask/src/deps.rs` holds the allowlist and makes this the only crate that may reach
`libghostty-vt` and `libghostty-vt-sys`.

Third-party crates: `libghostty-vt` at the pinned git rev with default features off
(no kitty graphics), `thiserror` and `tracing` (the cross-check warnings and the
factory error). Tests add `bytes`, `pretty_assertions`, `efr-screen` with its
`conformance` feature, and `efr-render` (a dev-dependency only, for its width count).

## Invariant

- No `unsafe` code. libghostty-vt is used through its safe API only. A future pin that
  needs a hand-written trampoline puts it in `src/ffi.rs` and adds that file to the
  tidy allowlist in the same commit.
- A `GhosttyScreen` is built, used and dropped on its actor thread. Its callbacks own
  what they touch (`Rc` buffers), so the terminal borrows nothing.
- A callback never blocks and never calls back into the terminal; the screen drains
  the buffers after libghostty-vt returns.
- The cross-check only logs. The scanner's marks stay the truth, and a feed always
  reaches the terminal in full.

## Tests

Run the tests of this crate alone (needs Zig on `PATH` at the version
`docs/ghostty-pin.md` pins; the first build clones ghostty into the target
directory):

```sh
cargo nextest run -p efr-screen-ghostty
```

or `just test-ghostty`. Unit tests drive libghostty-vt directly: the effect buffer,
the wire conversion of cells (styles, wide characters, grapheme clusters, erased
backgrounds), scrollback, GHOSTSNP round trips (including a snapshot cut inside an
escape sequence), the factories on a real `ScreenActor`, and the cross-check with
agreeing streams and with states libghostty-vt did not reach. `tests/it/conformance.rs`
runs the efr-screen conformance suite with the `ghostty` backend name; its rendered
screens are the `*__ghostty.snap` files in `crates/efr-screen/fixtures/vt/snapshots/`.
`tests/it/widths.rs` checks the width that `efr` counts in Ghostty
(`efr_render::text_width` by grapheme cluster) against the cursor of a real screen with
mode 2027 on: ZWJ emoji, flags, variation selectors, skin tones and wrapped lines. A
wrong count would make `efr` erase one row too many or too few when it draws its live
zone again.

A dev build compiles ghostty in Zig's Debug mode (Cargo sets `DEBUG=true`), which
takes in roughly 120 KB of plain text per second. The tests feed little, so they take
about a second; set `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe` for a faster local loop
over large inputs. No test needs the network once ghostty is cloned, reads
the user's home or sleeps on real time.
