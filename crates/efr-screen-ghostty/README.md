# efr-screen-ghostty

## Purpose

The libghostty-vt screen backend: `efr_screen::Screen` over a libghostty-vt
`Terminal`, the terminal emulator inside Ghostty. It is the only crate that links
libghostty-vt, so it is the only crate whose build needs Zig.

- `terminal`: `GhosttyScreen` and the factories `factory` and `restore_factory` that
  `ScreenActor::spawn` runs on the screen thread. libghostty-vt types are neither
  `Send` nor `Sync`, so the screen is built there with `Terminal::new` (never a
  borrowed allocator) and never leaves. A factory that cannot build its terminal logs
  the error and panics; the actor turns that into `ScreenError::Closed` for every
  handle. Every screen has the same settings: 5000 lines of scrollback and ghostty's
  own default colours (`#ffffff` on `#282c34`) for the OSC 10 and 11 answers.
- `effects`: the effect callbacks. libghostty-vt runs them inside `vt_write`, so they
  push answers to terminal queries (DA, DSR, DECRQM, OSC 10 and 11, in-band resize
  reports), bells and title changes into `RefCell` buffers, and the screen drains
  them into the `ScreenSink` after each feed and resize. The last OSC 133 step and the
  flags of a working directory change and of a full reset go into `Cell`s, which the
  cross-check reads and clears. A full reset (RIS) clears the title without a title
  callback, so its own callback reports an empty title when a title was set.
  libghostty-vt cuts a title at 1024 bytes, also inside a character; such a title
  keeps its whole characters.
- idle compression: `GhosttyScreen` asks the actor for an idle step 1 s after the last
  command. Each step is one `Terminal::compress(CompressionMode::Incremental)`, 1 ms
  apart, until the pass is done; then nothing happens until
  `Terminal::compression_activity` changes. A full scrollback of 160 columns goes from
  about 6.44 MiB to 0.51 MiB (see the bench `bench_perf_idle_compression`).
  libghostty-vt decompresses a page again when a snapshot reads it, and that does not
  change the activity, so a snapshot with scrollback starts a new pass.
- `Screen::text` comes from libghostty-vt's formatter in one call (plain text,
  soft-wrapped rows joined, trailing blanks trimmed), the text that Ghostty copies to
  the clipboard. It reads a cell only for the blank rows next to the cursor. The
  scrollback is counted first, so a full one costs no text. When libghostty-vt cannot
  count or format the rows, `efr_screen::cells_text` stands in; the two differ only
  at the wrap edges that `Screen::text` names, and
  `the_formatter_and_the_cells_differ_only_at_these_edges` tests each one.
- `snapshot`: the conversion to the wire `ScreenSnapshot` (visible rows, scrollback,
  cursor, title, alternate screen) and GHOSTSNP, libghostty-vt's own snapshot of the
  whole terminal, parser state included. `GhosttyScreen::encode_snapshot` writes it
  and `GhosttyScreen::restore` reads it back, here or in another process. It is kept
  for the attach of a client that runs libghostty-vt itself
  (`docs/adr/0004-ghostty-pin.md`); nothing calls it yet. The decoder keeps tracking
  the unfinished escape sequence of the snapshot, so a restored screen can make a
  snapshot again at once, also inside a sequence.
- `semantic_prompt`: the cross-check. The actor gives the screen the marks of its one
  scan with the bytes (`Screen::feed_marked`); the screen runs no scanner over the
  output. It reads its working directory back through a scanner only when an OSC 7
  mark comes, so that the same rules read both. The bytes go to libghostty-vt
  through `Terminal::vt_write_until`, which stops at the end of every OSC that the
  library reads, so each mark must end at such a stop. At the stop, the step that
  libghostty-vt reported (`Terminal::on_semantic_prompt`: the kind of prompt, the
  exit code), its working directory (`Terminal::pwd`) and the cursor row's prompt
  state must agree with the mark. libghostty-vt drops an OSC 133 whose body is longer
  than 2048 bytes and an OSC 7 longer than 2047, so a mark with a longer body is not
  checked. A
  disagreement is a `warn` log line; a feed never fails. A plain `Screen::feed`,
  which the actor never calls, is not checked.

The marks themselves never come from here: the actor's scanner in `efr-screen` is
the source of truth for every backend. libghostty-vt does not report `aid`, `cl`, the
fresh line of `A` or efr's end mark of a sandboxed call.

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
and `kitty-graphics` on (only for the setter that turns the protocol off), `thiserror`
and `tracing` (the cross-check warnings and the factory error). Tests add `bytes`,
`pretty_assertions`, `tracing-subscriber` (the log test), `efr-screen` with its
`conformance` feature, `efr-render` (a dev-dependency only, for its width count) and
`efr-test-support` (the bench helpers).

## Invariant

- `unsafe` only in `src/ffi.rs`, which is on the tidy allowlist. It sets the
  process-global log hook of libghostty-vt once, under a `Once`, and every path into
  libghostty-vt (`GhosttyScreen::new`, `snapshot::decode`, the test helper) passes
  that `Once` first. Everything else uses the safe API.
- The library log goes to `tracing` with the target `libghostty`, at `debug` for its
  errors and warnings and at `trace` for the rest, because the program in the hidden
  shell controls what the library logs. `EFR_LOG=info,libghostty=debug` shows it.
  The hook is set only when the filter shows that target at `debug` when the first
  screen starts; otherwise the library formats no log line. A later change of the
  filter does not change the hook. When libghostty-vt reports a failure that it did
  not handle (`Terminal::vt_processing_error`), the screen logs one warning.
- A `GhosttyScreen` is built, used and dropped on its actor thread. Its callbacks own
  what they touch (`Rc` buffers), so the terminal borrows nothing.
- A callback never blocks and never calls back into the terminal; the screen drains
  the buffers after libghostty-vt returns.
- The cross-check only logs. The scanner's marks stay the truth, and a feed reaches
  the terminal in full. Each chunk is scanned once, by the actor. The one exception:
  when libghostty-vt stops at the end of an OSC in a way that the binding cannot
  name, the bytes that it consumed are not known. Then the screen drops the rest of
  the chunk and logs a warning, because a second write would answer queries twice.
  The pinned library never stops that way.
- A hidden terminal draws nothing, so it does not say that it can. `configure` sets
  the Kitty image store to 0 (the protocol is off, and the `a=q` query gets no
  answer), turns the Glyph Protocol off, and keeps at most 8 KiB of one APC string.

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
agreeing streams and with states libghostty-vt did not reach. A screen test feeds
through `testing::Fed`, which scans each chunk as the actor does and calls
`Screen::feed_marked`, so it runs the checked path of efrd. `tests/it/conformance.rs`
runs the efr-screen conformance suite with the `ghostty` backend name; its rendered
screens are the `*__ghostty.snap` files in `crates/efr-screen/fixtures/vt/snapshots/`.
`tests/it/widths.rs` checks the width that `efr` counts in Ghostty
(`efr_render::text_width` by grapheme cluster) against the cursor of a real screen with
mode 2027 on: ZWJ emoji, flags, variation selectors, skin tones and wrapped lines. It
also checks that each glyph of efr's terminal look (`efr_render::Glyphs`: `⌂`, `›`,
`·`, `↪`, `⇄`, `▸`, `■`, `↳`, `↩`, `✓`, `✗`, `→`, `…`, `─`, the spinner and `•`) takes one
column, with mode 2027 on and off, as zle counts them in every locale. A
wrong count would make `efr` erase one row too many or too few when it draws its live
zone again.

A dev build compiles ghostty in Zig's Debug mode (Cargo sets `DEBUG=true`), which
takes in roughly 120 KB of plain text per second. The tests feed little, so they take
about a second; set `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe` for a faster local loop
over large inputs. No test needs the network once ghostty is cloned, reads
the user's home or sleeps on real time.

## Benches

The benches are ignored tests. `just bench-perf` runs them in release. They report
through `efr_test_support::bench`, a dev-dependency.

- `bench_perf_replay_screen` (`tests/it/replay_screen.rs`): the capture screens of a
  replay of 512 KiB and 5 MiB of output that moves the cursor, on vt100 and on ghostty.
  It uses the split rule of efr-shell's replay, and reads each screen with
  `Screen::text` and the limit of 1,000 rows, as efr-shell does, and with a snapshot
  of 1,000 rows as a baseline. It reports the time, the screens, and the bytes of
  text or the history rows. It fails when the text of the ghostty replay takes
  `GHOSTTY_TEXT_BUDGET` (30) percent or more of the time of its snapshot baseline;
  the formatter takes about 15 percent (the cells took about 50). The 5 MiB replay
  went from 572 ms to 166 ms with the formatter, and the 512 KiB one from 54.9 ms to
  13.0 ms.
- `bench_perf_idle_compression` (`src/terminal/tests.rs`): a 160x48 screen with 5000
  lines of build output, then its idle steps until they are done. It reports the
  resident bytes that libghostty-vt counts and the RSS of the process before and
  after, the steps, the pages and the time of all steps.
