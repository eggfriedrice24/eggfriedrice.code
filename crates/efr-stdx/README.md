# efr-stdx

## Purpose

Small extensions of `std` that every efr crate may use. This crate is the one place
where the workspace touches ambient process state: the wall clock, timers, OS
randomness, environment variables, child processes and the XDG directories.

Modules:

- `env`: typed access to the `EFR_*` variables below; the only reader of the process
  environment.
- `paths`: `Dirs { config, data, state, runtime }`, each `<XDG base>/efr` through
  `etcetera` unless an `EFR_*_DIR` variable replaces it, and the socket, `daemon.json`
  and lock file paths.
- `time`: the `Clock` trait (`now`, `sleep`, `timeout`) and `SystemClock`, the only
  caller of `SystemTime::now` and `tokio::time::sleep`.

## Tier

Tier 0, the bottom of the workspace. Every other crate may depend on it.

## Allowed dependencies

No workspace crate, ever. Every crate depends on this one, so an edge out of it would
make a cycle or pull a heavy crate into every build. `xtask/src/deps.rs` holds the
empty allowlist.

Third-party crates: `etcetera`, `jiff`, `thiserror`, `tokio`.

## Invariant

Time, randomness, the environment and child processes enter the workspace only here.
`clippy.toml` denies `SystemTime::now`, `tokio::time::sleep`, `std::env::var` and
`Command::new` in every crate and names the replacement in this one. The replacements
are traits (`Clock`, `Rng`) or narrow functions (`env::var`, `process::command`), so
other crates receive time and randomness by injection and their tests never wait on
real time.

## Environment variables

`efr_stdx::env::Var` lists every variable that efr reads, and a test checks that this
table names the same set. An empty value counts as unset.

| Variable | Meaning |
|---|---|
| `EFR_LOG` | The tracing filter for `efrd` and `efr`, in `EnvFilter` syntax. |
| `EFR_SCREEN` | The screen backend: `vt100` or `ghostty`. |
| `EFR_CONFIG_DIR` | An absolute path that replaces `$XDG_CONFIG_HOME/efr`. |
| `EFR_DATA_DIR` | An absolute path that replaces `$XDG_DATA_HOME/efr`. |
| `EFR_STATE_DIR` | An absolute path that replaces `$XDG_STATE_HOME/efr`. |
| `EFR_RUNTIME_DIR` | An absolute path that replaces `$XDG_RUNTIME_DIR/efr`. |
| `EFR_OPEN_BROWSER` | A flag. When it is on, `efr login openai` opens the login URL in a browser. |
| `EFR_RECORD_TRANSCRIPT` | An absolute path. `efrd` writes an NDJSON transcript of provider traffic and PTY bytes to it. |
| `EFR_TEST_ZSH` | A flag. Tests that drive a real zsh run only when it is on. |

A flag accepts `1`, `true`, `yes` or `on` for on and `0`, `false`, `no` or `off` for
off, in any letter case. Any other value is an error.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-stdx
cargo test -p efr-stdx --doc
```

The tests use no network, no real-time sleeps and no Zig.
