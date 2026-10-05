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
  exit code; `ToolOutputSink` hears a call's output while it runs, each change of
  whether its command waits for input (`input_changed`, ignored by default), and is
  asked whether a person can answer hidden input now (`can_answer_hidden`, true by
  default; the daemon answers it).
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
  network (`curl`, package installs and syncs but not `pacman -Q` or `-Ss`, nor the
  scripts of `npm run`, `npm test` and the like, `git pull`); both are a heuristic over the program names, and the engine judges the
  command line itself too. It declares the directory the line starts in (the hidden
  shell's, or the user's for a new shell), so a rule may allow a command in one
  project only. It also declares the paths the line names, so the path
  rules judge what a freely allowed read-only command reads: `shell_tool/words.rs`
  splits the line (quotes read as zsh reads them, the commands inside `$(...)` and
  groups included) and never fails, `shell_tool/reads.rs` says which words of each
  program are paths (every operand and every path-like option value; nothing for
  `echo`, `printf`, `basename` and the like; everything below the paths of `rg`, `grep
  -r`, `find`, `du`, `tree`, `ls -R` and `diff`, and the working directory when such a
  search names no path, with an unknown option failing closed),
  `shell_tool/writes.rs` says which words of a writer program it writes (every
  operand of `rm`, `rmdir`, `mkdir`, `touch`, `mv`, `chmod`, `truncate` and `tee`;
  the last operand or the `-t` directory of `cp`; the link of `ln`; every operand when
  the text cannot show which one `-t` takes), `reads.rs` does the same for `git rm`,
  `git mv` and `git worktree add`, and `shell_tool/declare.rs` resolves them against the hidden shell's directory (with
  `$HOME` at the start of a word read as `~`, and `rev:path` also naming `path`), follows
  `cd` within the line, turns a glob into everything below its fixed directory and an
  output redirection into a write. So `cat ~/.ssh/id_ed25519` declares the key and is
  denied, `rg TOKEN ~/.aws` declares `~/.aws` with everything below and asks, and
  `cp x ~/.config/efr/config.toml` declares a write of efr's config, which every mode
  denies. The engine's `auto` mode lets a writer program run because its writes are
  declared: the path rules decide them.
  What the text cannot show, such as the files a script opens, it cannot declare. The
  answer
  ends with `[exit code N, cwd DIR]`; a command still running at the timeout gets the
  screen's last lines and a note that the next call waits for it. The run names the
  call (`RunRequest::call`), so only answers for this call reach its command, and the
  tool passes the run's input waits and the question who can answer to its
  `ToolOutputSink`. The texts the model reads never promise a screen: the user does
  not see the hidden shell, and can answer a waiting command in their terminal only
  while they follow the turn. A full-screen program at the timeout
  (`Completion::FullScreen`) has its own text: the user is never asked about one and
  cannot reach it yet, so the model reads that, not that the user missed a question.
  A command that waited for hidden input that nobody could
  answer (`Completion::Unanswered`) is an error result that says efr interrupted it and
  that the user should run it in their own terminal or follow the turn while the model
  tries again. A busy shell, a
  shell that did not reach its prompt, a shell that exited and a command that cannot
  be typed are error results with advice for the model; other shell failures are
  `ToolError::Shell`. The advice is only what is true: nothing clears a busy shell from
  a turn (an interrupt reaches the shell only for a call in flight), so the model is
  told to tell the user, and that a new conversation gets a fresh shell.
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

A shell command cannot be refused that way, because `/bin`, `/etc/resolv.conf` and
many other paths are links. `ToolRequirements::with_real_paths` adds, after each
declared path that reaches the file system through a link, the path it reaches with
the same access, so `cat notes`, where `notes` links to `~/.ssh/id_ed25519`, is judged
as a read of the key too, and a recursive search whose root is a link is judged by its
target as well. It is the one step that reads the file system (it blocks; the daemon
runs it on the blocking pool before the engine decides). A link that a glob expands
to, or one below the root of a recursive search that the program follows, is not
seen.

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
