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
  working directory for a test and refuses a path that could leave the tree. The root is
  the real path of the temporary directory, so code that canonicalizes a path gets back
  the path the test holds. `TestDirs::new_in` puts the tree below another directory,
  such as cargo's target temp dir, for a test of the `auto` sandbox, which replaces the
  host's `/tmp` with a private one.
- `ndjson`: `Transcript`, the reader and validator of NDJSON transcripts, one `Record`
  per line, each `Entry` with its line number. The kinds are those of the structure
  document: `expect_outbound` `provider_request` and `event`, `emit_inbound`
  `provider_sse` and `client_frame`, `pty_bytes` in either direction (bytes the PTY
  produces come in, bytes written to it go out), and `clock_advance` with `ms` and no
  direction. The reader is strict: an unknown member, a wrong direction, a body of the
  wrong JSON type or bad base64 fails with the line number, because a fixture with a
  typo would otherwise test nothing. `to_ndjson` writes a transcript back with `dir`
  and `kind` first. What a body means is checked by the consumer of the record.
- `store`: `TestStore::open(clock)`, the real `efr-store` over a new private in-memory
  database: the same migrations, writer and read functions as the daemon's, with event
  times from the clock the test passes. PTY recordings, which are files, go to a
  temporary directory removed with the value. `events()` reads the whole log.
- `fixtures`: `fixtures::path(file!(), "case.ndjson")` is the file in the `fixtures/`
  directory of the crate that holds the calling test, found from `file!()` and never
  from the current directory. `crate_dir` and `dir` give the crate and its fixture
  directory. The file need not exist yet, so a bless step can write it.
- `redact`: `Redactor` replaces the values of one run with placeholders: the
  temporary working directory (`<CWD>`), the scratch path (`<SCRATCH>`), the host name
  (`<HOSTNAME>`), the temporary root (`<TMP>`) and RFC 3339 timestamps
  (`<TIMESTAMP>`). Values match only as whole names, and the longer of two overlapping
  values wins. `restore` puts the real paths back into inbound records. Both work on
  text and on every string and key of a JSON value. `TestDirs::redactor` starts one
  with the temporary root registered.
- `replay_provider`: `ReplayProvider`, an `efr_provider::Provider` driven by a
  transcript. Each `provider_request` record and the `provider_sse` records after it
  form one exchange. Each call to `stream` takes the next exchange, compares the
  request with the record as JSON after redaction on both sides, and streams the
  answer with placeholders restored. Records of other kinds are left to the scenario
  driver of `efr-test-daemon`. The answer format is canonical: each SSE `data` field
  is an `efr_provider::ProviderEvent` in its serde form, and an event of type `error`
  ends the answer with a `ProviderError` (`unauthorized` with an optional `message`,
  `rate_limited`, `not_logged_in`, `incomplete`, `overloaded` or `api`). An `api` error goes through
  `ProviderError::api`, as a real provider's does, so the code
  `context_length_exceeded` or the status 413 gives `ContextOverflow`. A request that
  does not match, or that
  comes after the last exchange, fails with `ProviderError::Api` and code
  `replay_mismatch` or `replay_exhausted`, not with a panic, because the code under
  test may run in a task whose panic the test never sees. `finish()` then reports the
  line, the JSON pointer of the first difference and both bodies, or the exchanges
  that were never requested. `paced()` holds each `provider_sse` record until the
  harness reports through `handled_through(line)` that it has handled the records
  before it, for an interrupt in the middle of an answer. The transcript is validated
  when the provider is built, so a broken fixture fails before the test runs. A
  provider's own wire format, such as the Responses API, is replayed at the HTTP level
  with wiremock instead.
- `wait`: `Wait`, a poll of a condition that another task or thread makes true:
  `Wait::new("the notice").until(|| ...)`, `until_some` for a value, `until_some_async`
  for a check that awaits, and `until_blocking` for a thread outside the runtime. The
  first polls only yield, later ones sleep 2 ms, and after a real time limit
  (`WAIT_LIMIT`, 10 s, or `limit(..)`) the wait fails with `TestSupportError::TimedOut`,
  which names what it waited for. A test uses it in place of a loop that counts yields,
  which passes on a fast machine and fails on a busy CI runner. A check that something
  did not happen still lets the other tasks run for a while and then looks once.
- `error`: `TestSupportError`, the crate's one error type.

## Tier

Tier 1. Kind `dev`: crates name it under `[dev-dependencies]` only, and no shipped
binary links it.

## Allowed dependencies

`efr-protocol`, `efr-store`, `efr-provider` and `efr-stdx`. `xtask/src/deps.rs` holds
the allowlist.

Third-party crates: `async-trait`, `base64`, `futures`, `jiff`, `serde`, `serde_json`,
`tempfile`, `thiserror` and `tokio`.

## Invariant

- This crate never depends on `efr-daemon`, `efr-cli` or a holder binary, directly or
  through another crate (a forbidden edge in `xtask/src/deps.rs`). `TestDaemon` and the
  scenario driver live in `efr-test-daemon`, so a leaf crate's tests never compile the
  daemon and never link two copies of a library.
- Nothing here reads the wall clock. The one wait on real time is the limit of `Wait`,
  which decides only when a test that would fail stops waiting, never the result of a
  test that passes; code under test still takes a `Clock`.
- The `TestRng` sequence never changes: fixtures hold ids made from it, and a test pins
  its first values.
- The transcript format is defined here, in `ndjson`. A recorder in a shipped binary
  may not depend on this crate, so it must write the same shape;
  `Transcript::to_ndjson` is the reference for it.

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
tests find `fixtures/transcripts/single_exchange.ndjson` from `file!()`. The store tests
append through the real writer and read events and a recording back. The redaction tests
cover whole-name matching, the timestamp grammar in both directions, and a proptest that
`restore` undoes `redact`. The replay provider tests replay the fixture through
`Arc<dyn Provider>`, check the failure reports, map every error event, validate broken
transcripts, and step a paced answer with `handled_through`. The wait tests see a
condition made true by a task, by a thread after real time and through an awaited
check, and check the error at a short limit. They use no network and no Zig, and only
the wait tests sleep, for at most a third of a second.
