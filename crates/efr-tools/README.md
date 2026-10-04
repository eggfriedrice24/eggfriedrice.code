# efr-tools

## Purpose

The tools the model calls, and the registry that offers them.

- `Tool`: `spec()` (a `ToolSpec`: name, description and the input's JSON Schema,
  generated with schemars from the input type, without `$schema` and `title`),
  `requirements(ctx, input)`, `preview(ctx, input)` (what a call would change, for
  its approval; none by default) and `invoke(ctx, input, out)`.
- `ToolRequirements`: every path a call touches with its `AccessMode` (read, read with
  everything below, or write), the command line it runs, and whether it talks to the
  network or may wait for input at the terminal. `requirements` is pure: paths are
  resolved lexically (`~` and `~/` under the home directory, relative paths under the
  user's working directory, or the hidden shell's for a command, `.` and `..` folded),
  so the conversation can ask before anything runs. Its `Debug` shows the command's
  length, not its text.
- `ToolRegistry`: `register`, `specs` (for the provider request, in registration
  order), and `requirements`, `preview` and `invoke` by name. It never hands out a
  tool.
- `write_file` previews a write as a unified diff against the current file (every
  line added for a new file), at most 200 lines and 16 KiB, with a line that says
  how much is left out.
- `ToolContext`: the call's ids (`CallIds`), the user's working directory, where the
  hidden shell is now (`shell_cwd`, when one runs), `$SCRATCH`, the scope, the origin,
  the home directory (`efr_scope::Home`), the clock and the write journal.
- `ToolResult`: the output the model sees, the truncation flag, the error flag and the
  exit code; `ToolOutputSink` hears a call's output while it runs.
- `truncate_middle`: the head and the tail of a long output with a
  `[... N bytes omitted ...]` line between them, cut on character boundaries and near
  line ends; `DEFAULT_OUTPUT_LIMIT` is 32 KiB.

The tools:

- `ShellTool` (`shell`) runs a command line in the conversation's hidden zsh through
  `efr_shell::CommandRunner` (the daemon passes its `ShellSessions`). Input:
  `command`, `timeout_seconds` (default 30, at most 600) and `nested_shell` (sentinel
  mode, for a `sudo -i`, `bash` or `ssh` started inside the hidden shell). A new shell
  starts in the user's working directory. It declares the command line, `interactive`
  when a program of the line may wait for input (`sudo`, `ssh`, an editor, a pager) or
  the call targets a nested shell, and `network` when a program usually reaches the
  network (`curl`, package installs and syncs but not `pacman -Q` or `-Ss`, `git
  pull`); both are a heuristic over the program names, and the engine judges the
  command line itself too. It declares the directory the line starts in (the hidden
  shell's, or the user's for a new shell), so a rule may allow a command in one
  project only. It also declares the paths the line names, so the path
  rules judge what a freely allowed read-only command reads: `shell_tool/words.rs`
  splits the line (quotes read as zsh reads them, the commands inside `$(...)` and
  groups included) and never fails, `shell_tool/reads.rs` says which words of each
  program are paths (every operand and every path-like option value; nothing for
  `echo`, `printf`, `basename` and the like; everything below the paths of `rg`, `grep
  -r`, `find`, `du`, `tree`, `ls -R` and `diff`, and the working directory when such a
  search names no path, with an unknown option failing closed), and
  `shell_tool/declare.rs` resolves them against the hidden shell's directory, follows
  `cd` within the line, turns a glob into everything below its fixed directory and an
  output redirection into a write. So `cat ~/.ssh/id_ed25519` declares the key and is
  denied, and `rg TOKEN ~/.aws` declares `~/.aws` with everything below and asks.
  What the text cannot show, such as the files a script opens, it cannot declare. The
  answer
  ends with `[exit code N, cwd DIR]`; a command still running at the timeout gets the
  screen's last lines and a note that the next call waits for it. A busy shell, a
  shell that did not reach its prompt, a shell that exited and a command that cannot
  be typed are error results with advice for the model; other shell failures are
  `ToolError::Shell`.
- `ReadFileTool` (`read_file`) reads a text file, whole or a range of lines (`offset`
  from 1, `limit`). It declares the path for reading and refuses a file over 16 MiB, a
  binary file (a NUL byte in the first 8 KiB) and anything that is not a regular file.
- `WriteFileTool` (`write_file`) writes the whole content of a file, creating missing
  parent directories. It declares the path for writing. Before it writes, it records
  the original in the context's `WriteJournal` as a `JournalEntry` with a
  `FileSnapshot` (path, mode, owner, group and content, or `Original::Missing`), and it
  writes nothing when the journal fails, so `undo turn N` can be built on the journal
  later. The write is atomic (`efr_stdx::fs::write_atomic`); an existing file keeps its
  mode, and its owner and group where the system allows it; a new file gets mode 0644.
  A file over 16 MiB is not replaced. The daemon persists the entries;
  `MemoryJournal` keeps them in memory. No content hash is taken yet: `blake3` comes
  with the undo command.

Both file tools refuse a path that goes through a symbolic link, and name the real
path in the error, because the permission engine judged the path as written: a
second call with the real path is judged on its own. A home reached through a link
(`/home` to `/var/home`) is not refused, since the engine knows both forms of it.

## Tier

Tier 2. The only edge inside the tier is `efr-tools -> efr-shell`.

## Allowed dependencies

`efr-shell` (the `CommandRunner` trait and its request and result types),
`efr-scope` (`Home`, the home directory in both forms), `efr-protocol` (ids, `Scope`,
`Origin`) and `efr-stdx` (`Clock`, atomic writes). `xtask/src/deps.rs` holds the
allowlist and forbids `efr-tools -> efr-permissions`, directly or through any chain.

Third-party crates: `schemars`, `serde` and `serde_json` (inputs and schemas), `tokio`
(the blocking pool for file work), `async-trait`, `thiserror` and `tracing`.

## Invariant

This crate knows nothing about permissions. Tools declare what a call needs, the
engine in `efr-permissions` decides, and `efr-conversation/src/turn.rs` enforces it
at the single check point before it calls `ToolRegistry::invoke`. The registry never
returns a tool handle, so the engine and the model reach the hidden shell only
through a `shell` call that passed that check.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-tools
```

The file tools run against temporary directories (a home and a working directory,
with symbolic links made where a test needs one); the shell tool runs against a fake
`CommandRunner` that scripts results and progress, and tables cover the split, the
paths each program reads and the declared paths with `cd` and globs. No test starts a
shell, uses the network or touches the user's home.
