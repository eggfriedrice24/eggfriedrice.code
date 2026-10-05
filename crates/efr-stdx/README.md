# efr-stdx

## Purpose

Small extensions of `std` that every efr crate may use. This crate is the one place
where the workspace touches ambient process state: the wall clock, timers, OS
randomness, environment variables, child processes and the XDG directories.

Modules, in the order of the milestone 1 file map:

- `paths`: `Dirs { config, data, state, runtime }` and the socket, `daemon.json` and
  lock file paths. Each root is its `EFR_*_DIR` variable, else `$EFR_HOME/<root>`,
  else `<XDG base>/efr` through `etcetera`; the runtime root falls back to
  `/run/user/<uid>/efr` when `XDG_RUNTIME_DIR` is unset and that directory is the
  user's own with mode 0700. `Dirs::resolve_with_sources` also says where each root
  came from (`RootSource`), for `efr paths` and `admin.status`.
  `Dirs::checked_socket_path` refuses a socket path longer than the 107 bytes a Unix
  socket holds.
- `time`: the `Clock` trait (`now`, `sleep`, `timeout`) and `SystemClock`, the only
  caller of `SystemTime::now` and `tokio::time::sleep`.
- `rng`: the `Rng` trait and `SystemRng`, a ChaCha12 generator seeded once from the
  operating system.
- `process`: `command(program, cwd)`, the only constructor of a child process. It
  sets the working directory and `PWD`, and removes the systemd variables of the
  daemon's own unit and the private variables below.
- `fs`: `write_atomic` (temporary file, flush, rename, flush the directory),
  `create_private` (a new file with mode 0600), `claim_dir` (a non-recursive
  `mkdir` with mode 0700, where an existing entry means taken) and `LinkedFile`, the
  file that `config.toml` and the project registry are changed through: read and
  written behind a symbolic link (a link to nothing is refused), and written only when
  it did not change since it was read.
- `env`: typed access to the `EFR_*` variables below; the only reader of the process
  environment.
- `thread`: `spawn_named(name, stack_size, f)`, a named std thread.
- `id`: `uuid_v7(clock, rng)`, a version 7 UUID whose time and random bits both come
  from the injected `Clock` and `Rng`.

The error type of every module is `StdxError`.

## Tier

Tier 0, the bottom of the workspace. Every other crate may depend on it.

## Allowed dependencies

No workspace crate, ever. Every crate depends on this one, so an edge out of it would
make a cycle or pull a heavy crate into every build. `xtask/src/deps.rs` holds the
empty allowlist.

Third-party crates: `etcetera`, `jiff`, `rand`, `rustix` (the user id, for the
`/run/user/<uid>` fallback), `thiserror`, `tokio`, `uuid`.

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
| `EFR_HOME` | An absolute path below which every root lives, in `config/`, `data/`, `state/` and `runtime/`. Each root's own variable wins over it. |
| `EFR_CONFIG_DIR` | An absolute path that replaces `$XDG_CONFIG_HOME/efr`. |
| `EFR_DATA_DIR` | An absolute path that replaces `$XDG_DATA_HOME/efr`. |
| `EFR_STATE_DIR` | An absolute path that replaces `$XDG_STATE_HOME/efr`. |
| `EFR_RUNTIME_DIR` | An absolute path that replaces `$XDG_RUNTIME_DIR/efr`. |
| `EFR_OPEN_BROWSER` | A flag. When it is on, `efr login openai` opens the login URL in a browser. |
| `EFR_TEST_ZSH` | A flag. Tests that drive a real zsh run only when it is on. |
| `EFR_MODE` | The permission mode that `efr send`, `efr new` and `efr settings` ask for: `manual`, `cautious` or `auto`. A flag wins over it. The zsh plugin hands over the terminal's choice in it. |
| `EFR_MODEL` | The model that `efr send`, `efr new` and `efr settings` ask for. A flag wins over it. The zsh plugin hands over the terminal's choice in it. |
| `EFR_EFFORT` | The reasoning effort that `efr send`, `efr new` and `efr settings` ask for. A flag wins over it. The zsh plugin hands over the terminal's choice in it. |
| `EFR_CONTEXT` | Private. The shell context JSON that the zsh plugin hands to `efr send` and `efr new`. |
| `EFR_LAST_COMMAND` | Private. The last command line of the user's shell, from the zsh plugin to `efr send` and `efr new`. |
| `EFR_PROMPT` | Private. The prompt that the zsh plugin hands to `efr send` and `efr new`. |

A flag accepts `1`, `true`, `yes` or `on` for on and `0`, `false`, `no` or `off` for
off, in any letter case. Any other value is an error.

The private variables (`Var::PRIVATE`) carry what the user typed. They travel in the
environment and not as arguments because any local user can read a command line in
`/proc/<pid>/cmdline`, while `/proc/<pid>/environ` is readable only by the process's
own user. `process::command` removes them from every child, and `Env`'s `Debug` shows
their values only by length.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-stdx
cargo test -p efr-stdx --doc
```

The tests use no network, no real-time sleeps and no Zig.
