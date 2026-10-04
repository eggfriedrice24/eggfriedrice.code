# efr-stdx

## Purpose

Small extensions of `std` that every efr crate may use. This crate is the one place
where the workspace touches ambient process state: the wall clock, timers, OS
randomness, environment variables, child processes and the XDG directories.

## Tier

Tier 0, the bottom of the workspace. Every other crate may depend on it.

## Allowed dependencies

No workspace crate, ever. Every crate depends on this one, so an edge out of it would
make a cycle or pull a heavy crate into every build. `xtask/src/deps.rs` holds the
empty allowlist.

Third-party crates: `thiserror`.

## Invariant

Time, randomness, the environment and child processes enter the workspace only here.
`clippy.toml` denies `SystemTime::now`, `tokio::time::sleep`, `std::env::var` and
`Command::new` in every crate and names the replacement in this one. The replacements
are traits (`Clock`, `Rng`) or narrow functions (`env::var`, `process::command`), so
other crates receive time and randomness by injection and their tests never wait on
real time.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-stdx
cargo test -p efr-stdx --doc
```

The tests use no network, no real-time sleeps and no Zig.
