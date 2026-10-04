# efr-holder

## Purpose

The PTY holder contract. The daemon needs pseudo-terminals with a hidden zsh on each,
and it must not care who holds them: at milestone 1 the holder is
`efr_pty::LocalPtyHolder` inside the daemon; at milestone 5 it is `efr-ptyd`, a
separate service that keeps the shells alive across daemon restarts. This crate is
the boundary between the two sides: types and a trait, no IO.

Modules:

- `spec`: `SpawnSpec`, what to run on a new PTY. The caller mints the `PtyId`; the
  program and the working directory are absolute; the child gets exactly the spec's
  environment and inherits nothing from the holder process. `SpawnSpec::validate`
  checks all of that, and holders call it before they open anything, because a spec
  that arrives over a socket bypasses the constructor. `Debug` shows environment names
  without their values.
- `error`: `HolderError`, the crate's one error type.

## Tier

Tier 1.

## Allowed dependencies

`efr-protocol` (for `PtyId` and `Size`) and `efr-stdx`. The crate uses only
`efr-protocol` so far: nothing here reads a clock, draws randomness or touches the
file system. `xtask/src/deps.rs` holds the allowlist.

Third-party crates: `serde` and `thiserror`.

## Invariant

- No IO and no `unsafe`: the crate holds types and a trait. Opening a PTY and the
  `pre_exec` code belong to `efr-pty` (the only `unsafe` at milestone 1), and passing
  descriptors over a socket to `efr-fdpass` (milestone 5).
- A spawn is fully described by its spec: an absolute program (no `PATH` search), an
  absolute working directory and the child's whole environment. The same spec starts
  the same shell in the daemon and in `efr-ptyd`.
- Environment values never reach a `Debug` string or an error, because the hidden
  shell's environment is copied from the user's and can hold tokens.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-holder
```

The tests are pure: decision tables for `SpawnSpec::validate` and JSON round trips.
They use no network, no real-time sleeps and no Zig.
