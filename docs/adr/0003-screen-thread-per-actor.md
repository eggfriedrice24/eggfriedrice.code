# 0003: Every screen runs on its own std thread

Status: accepted, 2026-10-03.

## Context

libghostty-vt types are `!Send + !Sync` by design: the C library may use thread-local
state, and every effect callback runs synchronously inside `vt_write` on the calling
thread. `tokio::spawn` and `spawn_blocking` need `Send`, and a `LocalSet` would still
need a thread of its own. Large scrollback work can stall.

## Decision

- The `Screen` trait has no `Send` bound.
- `ScreenActor::spawn` starts a named std thread (`screen-<conversation short id>`,
  512 KiB stack) and builds the screen inside it from a `Send` factory.
- Commands arrive over a bounded `std::sync::mpsc::sync_channel(64)`. Replies to
  terminal queries are buffered inside the callback and leave after each feed as
  `ScreenEvent::PtyReply` on a tokio channel to the shell's async PTY writer, together
  with shell marks, title changes, bells and snapshots.
- vt100 screens use the same actor and factory.

Non-goals: no `Mutex<Terminal>`, no `spawn_blocking` with a `Terminal`, no `LocalSet`
inside the main runtime, no `Screen` built outside its actor thread.

## Consequences

- N+1 threads for N open conversations; idle shells are reaped with their threads.
- One slow screen cannot stall the others. A single shared screen thread was rejected
  for that reason.
- A callback that blocks shows up as a conformance test timeout, not a production hang.
