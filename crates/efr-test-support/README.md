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
- `rng`: `TestRng::new(seed)`, an `efr_stdx::rng::Rng` over SplitMix64. The same seed
  gives the same ids, PKCE verifiers and scratch names on every run and every machine.
  The algorithm is written out here, not taken from `rand`, because `rand` does not
  promise to keep a seeded sequence across versions; a test pins the first values.
- `dirs`: `TestDirs`, a throwaway tree with the four efr roots (`config/`, `data/`,
  `state/`, `runtime/`, the layout `just run` uses) and a stand-in `home/`, all mode
  0700, removed on drop. `dirs()` gives the `efr_stdx::paths::Dirs`, `env()` an
  `efr_stdx::env::Env` that names them through `EFR_*_DIR`, and `create_dir` makes a
  working directory for a test. The root is the real path of the temporary directory,
  so code that canonicalizes a path gets back the path the test holds.
- `ndjson`: `Transcript`, the reader and validator of NDJSON transcripts, one `Record`
  per line, each `Entry` with its line number. The kinds are those of the structure
  document: `expect_outbound` `provider_request` and `event`, `emit_inbound`
  `provider_sse` and `client_frame`, `pty_bytes` in either direction (bytes the PTY
  produces come in, bytes written to it go out), and `clock_advance` with `ms` and no
  direction. The reader is strict: an unknown member, a wrong direction, a body of the
  wrong JSON type or bad base64 fails with the line number, because a fixture with a
  typo would otherwise test nothing. `to_ndjson` writes a transcript back with `dir`
  and `kind` first. What a body means is checked by the consumer of the record.
- `fixtures`: `fixtures::path(file!(), "case.ndjson")` is the file in the `fixtures/`
  directory of the crate that holds the calling test, found from `file!()` and never
  from the current directory. `crate_dir` and `dir` give the crate and its fixture
  directory. The file need not exist yet, so a bless step can write it.
- `error`: `TestSupportError`, the crate's one error type.

## Tier

Tier 1. Kind `dev`: crates name it under `[dev-dependencies]` only, and no shipped
binary links it.

## Allowed dependencies

`efr-protocol`, `efr-store`, `efr-provider` and `efr-stdx`. `xtask/src/deps.rs` holds
the allowlist.

Third-party crates: `base64`, `jiff`, `serde`, `serde_json`, `tempfile`, `thiserror` and
`tokio`.

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
`wait_for_sleeps`. The generator tests pin the published SplitMix64 values and one
UUIDv7 made from `TestClock` and `TestRng`. The transcript tests parse every record
form, run a decision table of invalid lines against the problem each one names, and
check with a proptest that any transcript survives writing and reading. The fixture
tests find `fixtures/transcripts/single_exchange.ndjson` from `file!()`. They use no
network, no real-time sleeps and no Zig.
