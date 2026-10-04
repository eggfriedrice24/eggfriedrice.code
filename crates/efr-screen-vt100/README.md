# efr-screen-vt100

## Purpose

The screen backend that needs no Zig. `Vt100Screen` implements `efr_screen::Screen`
over the pure-Rust `vt100` crate (0.16.2), and `factory(size)` builds one on its
screen thread for `ScreenActor::spawn`:

```rust
let (handle, events) = ScreenActor::spawn("screen-1a2b", efr_screen_vt100::factory(size), size)?;
```

Every test in the workspace and every build without libghostty-vt runs on this
backend, including the daemon when `efr-screen-ghostty` is not compiled in. It goes
through the same `ScreenActor` and the same conformance suite as the ghostty backend;
only the factory differs. Shell marks (OSC 133 and OSC 7) never come from here: the
actor's `ShellMarkScanner` finds them before the bytes reach the backend.

- `screen`: `Vt100Screen`, `factory` and `DEFAULT_SCROLLBACK_ROWS` (1000 rows; a
  vt100 cell is 32 bytes, so about 6 MiB per screen at 200 columns).
  `Vt100Screen::with_scrollback` sets another capacity. Snapshots read the scrollback
  through vt100's scrolled view a screen at a time and leave the view at the bottom.
- `cells`: vt100 cells to wire cells. The default colour is no colour, indexed and RGB
  colours map one to one, bold, italic, underline and inverse carry over, dim is
  dropped (the wire has no field for it), and the second cell of a wide character has
  no text.
- `recorder`: the `vt100::Callbacks` value. vt100 owns it for the life of the parser
  while a `ScreenSink` is lent to one `feed` call only, so it keeps bells and title
  changes in order and the screen hands them to the sink after each `process`. It
  also keeps the title and the OSC 7 URL.

### What vt100 does and does not do

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

### Guards against vt100 panics

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

Tier 2.

## Allowed dependencies

`efr-screen` only (the `Screen` and `ScreenSink` traits, and the wire types it
re-exports from `efr-protocol`). `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `vt100` (the terminal emulator) and `tracing` (the warning when a
vt100 panic is caught). The tests add `pretty_assertions`, `proptest` and
`efr-screen`'s `conformance` feature.

## Invariant

- A `Vt100Screen` reports exactly what vt100 renders, normalised by the actor like
  every backend. It never answers a query, so PTY replies come only from the ghostty
  backend.
- No input and no size makes `feed`, `resize` or `snapshot` panic out of the
  backend.
- Shell marks are not this crate's business; the scanner in `efr-screen` owns them.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-screen-vt100
```

Unit tests cover the cell conversion, the recorder (titles, URLs, limits, order) and
the screen (scrollback paging, the one-row view, the panic guard, the cursor at a
pending wrap). A proptest feeds random escape fragments between random resizes (sizes
0 to 6) and snapshots, and checks that nothing panics out of the screen.

`tests/conformance.rs` runs `efr_screen::conformance::run("vt100", factory)` over
every fixture in `crates/efr-screen/fixtures/`. Its rendered screens are the
`*__vt100.snap` files in `crates/efr-screen/fixtures/vt/snapshots/`; they match the
fake backend's except for the replies vt100 does not give. Nothing here needs Zig,
zsh, the network or real time.
