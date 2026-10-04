# efr-test-support

## Purpose

Light helpers for the tests of every efr crate, so that a test gets time, randomness,
directories, a store and provider traffic from one place and never from the machine.

- `clock`: `TestClock`, an `efr_stdx::time::Clock` that moves only when the test moves
  it. It starts at 2026-10-04T12:00:00Z (`TestClock::START`). `advance`, `set` and
  `advance_to_next_deadline` move it; a sleep ends when the clock reaches its deadline.
  `wait_for_sleeps(n)` waits, without real time, until the code under test is asleep,
  so a test does not race the task it drives. `requested_sleeps` lists every duration
  the code asked for, for backoff assertions.

## Tier

Tier 1. Kind `dev`: crates name it under `[dev-dependencies]` only, and no shipped
binary links it.

## Allowed dependencies

`efr-protocol`, `efr-store`, `efr-provider` and `efr-stdx`. `xtask/src/deps.rs` holds
the allowlist.

Third-party crates: `jiff` and `tokio`.

## Invariant

- This crate never depends on `efr-daemon`, `efr-cli` or a holder binary, directly or
  through another crate (a forbidden edge in `xtask/src/deps.rs`). `TestDaemon` and the
  scenario driver live in `efr-test-daemon`, so a leaf crate's tests never compile the
  daemon and never link two copies of a library.
- Nothing here reads the wall clock or waits on real time.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-test-support
```

The clock tests poll sleeps and timeouts by hand and drive one spawned task with
`wait_for_sleeps`. They use no network, no real-time sleeps and no Zig.
