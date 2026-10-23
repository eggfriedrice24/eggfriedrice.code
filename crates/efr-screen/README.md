# efr-screen

## Purpose

The screen model boundary. Everything that turns PTY output into a terminal screen
goes through this crate, whichever backend renders it:

- `screen`: the `Screen` trait (`feed`, `resize`, `snapshot`, `row`, `cursor`, `title`,
  `pwd`), implemented by the `vt100` module of this crate and by
  `efr-screen-ghostty`. It has no `Send` bound.
- `sink`: the `ScreenSink` trait through which a backend reports PTY replies (DA, DSR,
  DECRQM, OSC 10 and 11 answers), bells and title changes while it processes bytes.
- `actor`: `ScreenActor::spawn`, which runs one screen on its own named std thread,
  `ScreenHandle` (the only type other crates touch), `ScreenEvents` and `ScreenEvent`.
- `shell_marks`: `ShellMarkScanner`, the backend-independent scanner for OSC 133
  semantic prompts and OSC 7 working-directory reports. It runs on every chunk before
  the backend sees it, so both backends report identical `ShellMark`s, each with the
  recording offsets of its first and last byte. The shell tool's output for a command
  is `recording[OutputStart.end .. CommandEnd.start]`. It also reads efr's own end mark
  of a sandboxed call, `ESC ] 133 ; efr-sbx ; <nonce> BEL` with the nonce as exactly 32
  lowercase hex digits, as `ShellMarkKind::SandboxEnd { nonce }`. Any other body after
  `efr-sbx;` is no mark. The scanner only reads it; `efr-shell` compares the nonce
  with the call's own.
- `snapshot`: `ScreenCapture` and the normalisation of a backend's snapshot into the
  wire `efr_protocol::ScreenSnapshot`.
- The wire types a backend builds (`ScreenSnapshot`, `RowCells`, `Cell`, `Color`,
  `Cursor`, `Size`, `Seq`) are re-exported from `efr-protocol`, so a backend crate implements
  `Screen` with `efr-screen` as its only workspace dependency.
- `vt100`: the backend that needs no Zig (see "The vt100 backend" below).
- `conformance` (feature `conformance`, enabled only by test targets): the suite that
  drives the real `ScreenActor` with a backend factory over the NDJSON fixtures in
  `fixtures/vt/` and `fixtures/shell_marks/`.

Consumers: `efr-shell` (one screen per hidden shell, state from marks), `efr-daemon`
(backend selection, `pty.attach` snapshots) and later the PTY proxy.

### Threading model

libghostty-vt forces it, and the crate states it so nobody reintroduces a tokio task
that will not compile:

- All libghostty-vt types are `!Send + !Sync` by design: the C API may use
  thread-local state, every effect callback runs synchronously inside `vt_write` on the
  calling thread, and `compress` can stall on large scrollback. `tokio::spawn` and
  `spawn_blocking` require `Send`, and a `LocalSet` would still need its own thread.
- Therefore `ScreenActor::spawn(name, factory, size)` starts a named std thread
  through `efr_stdx::thread::spawn_named` and builds the `Screen` inside it from the
  `Send` factory. The screen is then resized to `size`, so a screen always starts at
  the size its owner asked for.
- Inbound: feed, resize, snapshot and shutdown commands go over a bounded
  `std::sync::mpsc::sync_channel(64)`. The async PTY reader task parks on
  backpressure: `ScreenHandle::feed` waits on a `tokio::sync::Notify` that the actor
  signals after every command it takes, so a full queue never blocks a tokio worker.
  The `_blocking` methods are for plain threads (the PTY proxy, the conformance
  suite).
- Outbound: the actor's sink buffers PTY replies while the backend runs; after each
  feed the actor drains the buffer into one `ScreenEvent::PtyReply` on a bounded
  `tokio::sync::mpsc` channel to the session's async PTY writer. Only the daemon
  answers terminal queries; clients never write replies. The same event stream carries
  `ShellMark`, `TitleChanged`, `Bell` and the results of `request_snapshot`. Per feed
  the order is: marks in stream order, then bells and title changes in the order they
  happened, then the reply bytes.
- The event channel is bounded (64): when its reader falls behind, the actor waits,
  the command queue fills and the PTY reader stops reading. That is the backpressure
  path, so the task that drains `ScreenEvents` must not wait on
  `ScreenHandle::snapshot` itself; it uses `request_snapshot`, whose result arrives in
  order on the event stream. When `ScreenEvents` is dropped, the actor discards
  events and keeps serving snapshots.
- The vt100 backend is `Send` in practice but implements the same trait and is
  spawned through the same factory, so the conformance suite exercises the identical
  actor code and only swaps the factory. A future blocking libghostty callback shows
  up as a conformance timeout (nextest's slow-timeout), not a production hang.
- Cost: one thread per hidden conversation shell plus one for the PTY proxy screen,
  N+1 threads for N open conversations. Each thread gets a 512 KiB stack
  (`SCREEN_STACK_SIZE`) because the actor loop is shallow. A single thread for all
  screens was rejected because one slow `compress` or large-scrollback `vt_write`
  would stall every conversation.
- The actor ends on `shutdown`, when every `ScreenHandle` is dropped, or when the
  backend panics. Then the event stream ends (`recv` returns `None`) and every handle
  method returns `ScreenError::Closed`; a sender parked on a full queue is woken.

### Non-goals

- No `Mutex<Terminal>`. wezterm can lock its terminal only because it is pure Rust and
  `Send`.
- No `spawn_blocking` with a `Terminal`.
- No `LocalSet` inside the main runtime.
- No `Screen` constructed outside its actor thread.
- The scanner does not interpret 8-bit C1 controls (`0x9D` and friends): the PTY
  stream is UTF-8, where those bytes are continuation bytes.
- The scanner does not validate an OSC 7 host against the local hostname; the
  consumer decides what a remote host means.

### Scanner rules

- `ESC ]` opens an OSC; `BEL` or `ESC \` closes it. A body longer than 4096 bytes is
  dropped up to its terminator. DCS, SOS, PM and APC strings are skipped up to `ESC \`,
  so their payload (a sixel image, a kitty graphics blob) is never read as an OSC.
  CAN and SUB abort any sequence, as in the VT parser.
- An `ESC` inside an OSC body that is not followed by `\` aborts the OSC without a mark
  and starts a new escape sequence. libghostty-vt and vt100 dispatch the OSC in that
  case; the scanner does not, so a stray fragment in binary output never produces a
  false mark and never swallows the next real one.
- Inside a skipped string, a doubled `ESC ESC` is payload: that is how tmux wraps
  passthrough sequences, so the marks of a shell running in tmux inside the hidden
  shell are not taken for the hidden shell's own. Any other `ESC` ends the string and
  starts a new sequence, as in the VT parser, so a stray `ESC P` in binary output
  cannot hide the next real mark.
- State survives chunk boundaries, so any split of a stream yields the same marks
  (proptest). When a chunk's base offset does not continue the previous chunk (a gap
  in the recording), a partly read sequence is dropped.
- OSC 133 actions `A`, `P`, `B`, `C` and `D`, with the options `aid`, `k`, `cl`,
  `err`, `cmdline` (printf `%q` quoting) and `cmdline_url` (percent encoding), and the
  exit code as the first value after `D;`. The first occurrence of an option wins and
  a malformed value is ignored, as in ghostty's parser. Other actions are not marks.
- OSC 7 in the forms `file://host/path` (percent-decoded, query and fragment cut) and
  `kitty-shell-cwd://host/path` (the raw path). An empty URL is not a mark.

### The vt100 backend

The screen backend that needs no Zig, in the module `efr_screen::vt100`.
`Vt100Screen` implements `Screen` over the pure-Rust `vt100` crate (0.16.2), and
`factory(size)` builds one on its screen thread for `ScreenActor::spawn`:

```rust
let (handle, events) = ScreenActor::spawn("screen-1a2b", efr_screen::vt100::factory(size), size)?;
```

Every test in the workspace and every build without libghostty-vt runs on this
backend, including the daemon when `efr-screen-ghostty` is not compiled in. It goes
through the same `ScreenActor` and the same conformance suite as the ghostty backend;
only the factory differs. Shell marks (OSC 133 and OSC 7) never come from here: the
actor's `ShellMarkScanner` finds them before the bytes reach the backend.

The backend has no cargo feature: every user of this crate (`efr-shell`, `efr-daemon`)
needs it, and the `vt100` crate is small and pure Rust. It was a crate of its own
until ADR 0014 folded it into this one.

- `vt100/screen`: `Vt100Screen`, `factory` and `DEFAULT_SCROLLBACK_ROWS` (1000 rows; a
  vt100 cell is 32 bytes, so about 6 MiB per screen at 200 columns).
  `Vt100Screen::with_scrollback` sets another capacity. Snapshots read the scrollback
  through vt100's scrolled view a screen at a time and leave the view at the bottom.
- `vt100/cells`: vt100 cells to wire cells. The default colour is no colour, indexed
  and RGB colours map one to one, bold, italic, underline and inverse carry over, dim is
  dropped (the wire has no field for it), and the second cell of a wide character has
  no text.
- `vt100/recorder`: the `vt100::Callbacks` value. vt100 owns it for the life of the parser
  while a `ScreenSink` is lent to one `feed` call only, so it keeps bells and title
  changes in order and the screen hands them to the sink after each `process`. It
  also keeps the title and the OSC 7 URL.

#### What vt100 does and does not do

- It answers no terminal queries (DA, DSR, DECRQM, OSC 10 and 11), so nothing ever
  reaches `ScreenSink::pty_reply`. The fixtures that check replies (`da1_reply`,
  `dsr_reply`) list `vt100` in their `differs`.
- It sends no in-band resize report (mode 2048), and it does not reflow on resize: a
  narrower grid cuts rows, and fewer rows drop the bottom rows instead of pushing the
  top ones into the scrollback.
- Lines that scroll inside a scroll region (DECSTBM) never enter the scrollback, even
  when the region starts at the top row.
- `pwd()` is the raw OSC 7 URL as the program sent it, the form libghostty-vt's
  `Terminal::pwd` returns. The title and the URL follow libghostty-vt's limits (1024
  and 4096 bytes) and an empty value clears them. vte splits an OSC at every `;`, so
  the recorder joins the pieces of a title or URL that holds one; vte keeps 16 pieces
  with the OSC number, so a value with more than 14 `;` loses its tail.

#### Guards against vt100 panics

Fuzzing vt100 0.16.2 found inputs that make it panic; none of them at 2 by 2 or
larger without a resize:

- A line that wraps on a grid of one row underflows a row index. A one-row screen
  therefore runs on two rows of vt100 and shows the row the cursor is on, so text that
  arrives line by line looks as it would on a single row. Moving the cursor up or
  down, a no-op on a real single row, switches between the two.
- A wide character on a grid of one column, and printing over the first half of a
  wide character that a narrower resize cut at the right edge, index past the row.
  `feed` catches the panic, logs a warning and starts over with a blank grid of the
  same size, keeping the title and the working directory; the rest of that chunk is
  lost. Without the guard the panic would end the screen actor and the shell would
  lose its screen for good.

A dimension of 0 becomes 1, because vt100 subtracts 1 from both when it builds or
resizes a grid.

## Tier

Tier 1.

## Allowed dependencies

`efr-protocol` (the wire `ScreenSnapshot`, `RowCells`, `Cursor`, `Size` and `Seq`) and
`efr-stdx` (the named thread builder). `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `tokio` (the event channel, `oneshot` and `Notify`), `bytes`,
`memchr` (finding `ESC` in long runs of output), `thiserror`, `vt100` (the terminal
emulator of the vt100 backend) and `tracing` (the warning when the vt100 backend
catches a vt100 panic). The `conformance`
feature adds `serde_json` (the fixture reader) and `insta` (one rendered-screen
snapshot per vt fixture per backend).

## Invariant

- A `Screen` is built on, used on and dropped on its own actor thread; other crates
  hold only `ScreenHandle` and `ScreenEvents`.
- OSC 133 and OSC 7 are parsed once, here, before any backend sees the bytes, so every
  backend produces the same marks with the same recording offsets. The conformance
  suite compares marks for every backend, even for fixtures that list the backend in
  their `differs` list.
- Nothing in the actor waits on real time; backpressure is the bounded queues.
- A `Vt100Screen` reports exactly what vt100 renders, normalised by the actor like
  every backend. It never answers a query, so PTY replies come only from the ghostty
  backend.
- No input and no size makes the vt100 backend's `feed`, `resize` or `snapshot` panic
  out of the backend.
- The vt100 backend uses only this crate's public surface (the traits and the wire
  types), as an outside backend crate would.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-screen
```

The unit tests of the actor use a small in-crate fake screen that is deliberately
`!Send`, which proves the actor needs no `Send` screen. The scanner has unit tests per
file and a proptest that checks that any split of a byte stream yields the same marks.
The conformance suite runs against the fake screen through a dev-dependency of the
crate on itself with the `conformance` feature.

The vt100 backend has unit tests for the cell conversion, the recorder (titles, URLs,
limits, order) and the screen (scrollback paging, the one-row view, the panic guard,
the cursor at a pending wrap). A proptest feeds random escape fragments between random
resizes (sizes 0 to 6) and snapshots, and checks that nothing panics out of the
screen.

The integration test binary runs the suite against the vt100 backend
(`tests/it/conformance.rs`), and `efr-screen-ghostty` runs it the same way against
libghostty-vt:

```rust
#[test]
fn vt100() {
    efr_screen::conformance::run("vt100", efr_screen::vt100::factory);
}
```

The vt100 backend's rendered screens are the `*__vt100.snap` files in
`fixtures/vt/snapshots/`; they match the fake backend's except for the replies vt100
does not give.

A fixture is one JSON object per line: a `case` header with the grid size and the
`differs` list, then `feed`, `resize` and `expect_*` records (see
`src/conformance/cases.rs`). A backend named in `differs` skips the rows, cursor,
size, title, bell and reply checks of that fixture, never the mark checks. The first
run of a new backend writes its snapshots as `fixtures/vt/snapshots/*.snap.new`;
review them and accept them with `just bless`. No test sleeps on real time, needs the
network or needs Zig.

## Benches

The benches are ignored tests. `just bench-perf` runs them in release. They report
through `efr_test_support::bench`, a dev-dependency.

- `bench_perf_osc_scan` (`tests/it/osc_scan.rs`): the time and the throughput of the
  mark scanner on 0.5, 5 and 50 MiB of plain output and of output with many escape
  sequences. It also scans prompt marks in chunks of 4093 and 7 bytes, so that the
  marks cross the chunk edges, and counts the marks that it finds.
