# efr-screen

## Purpose

The screen model boundary. Everything that turns PTY output into a terminal screen
goes through this crate, whichever backend renders it:

- `screen`: the `Screen` trait (`feed`, `resize`, `snapshot`, `row`, `cursor`, `title`,
  `pwd`), implemented by `efr-screen-vt100` and `efr-screen-ghostty`. It has no `Send`
  bound.
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
- `efr-screen-vt100` is `Send` in practice but implements the same trait and is
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

## Tier

Tier 1.

## Allowed dependencies

`efr-protocol` (the wire `ScreenSnapshot`, `RowCells`, `Cursor`, `Size` and `Seq`) and
`efr-stdx` (the named thread builder). `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `tokio` (the event channel, `oneshot` and `Notify`), `bytes`,
`memchr` (finding `ESC` in long runs of output) and `thiserror`. The `conformance`
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

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-screen
```

The tests use a small in-crate fake screen (the real backends are separate crates)
that is deliberately `!Send`, which proves the actor needs no `Send` screen. The
scanner has unit tests per file and a proptest that checks that any split of a byte
stream yields the same marks. The conformance suite runs against the fake screen
through a dev-dependency of the crate on itself with the `conformance` feature.

A backend crate runs the suite from `tests/it/conformance.rs`:

```rust
#[test]
fn conformance() {
    efr_screen::conformance::run("vt100", efr_screen_vt100::factory);
}
```

A fixture is one JSON object per line: a `case` header with the grid size and the
`differs` list, then `feed`, `resize` and `expect_*` records (see
`src/conformance/cases.rs`). A backend named in `differs` skips the rows, cursor,
size, title, bell and reply checks of that fixture, never the mark checks. The first
run of a new backend writes its snapshots as `fixtures/vt/snapshots/*.snap.new`;
review them and accept them with `just bless`. No test sleeps on real time, needs the
network or needs Zig.
