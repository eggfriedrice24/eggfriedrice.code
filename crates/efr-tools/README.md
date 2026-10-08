# efr-tools

## Purpose

The tools the model calls, and the registry that offers them: `shell`,
`read_file`, `write_file` and `apply_patch`.

- `Tool`: `spec()` (a `ToolSpec`: name, description and the input's JSON Schema,
  generated with schemars from the input type, without `$schema` and `title`; or,
  from `ToolSpec::freeform`, a freeform tool whose input is plain text that a
  `ToolGrammar` describes, with the schema of its function form, one string member
  `FREEFORM_INPUT` (`input`)),
  `requirements(ctx, input)`, `preview(ctx, input)` (what a call would change, for
  its approval; none by default) and `invoke(ctx, input, out)`.
- `ToolRequirements`: every path a call touches with its `AccessMode` (read, read with
  everything below, or write), the command line it runs, whether it talks to the
  network or may wait for input at the terminal, whether it deletes or moves a file
  (`destructive`, which `apply_patch` sets), and, for the shell tool, the model's
  `needs` (`efr_protocol::Needs`) and whether the line goes to a nested shell
  (`nested`), which the engine reads in the `auto` mode. The daemon adds the facts
  about the line's files and programs (`efr_permissions::CallFacts`) when it copies
  them into the engine's requirements, because this crate cannot name that type. `requirements` is pure: paths are
  resolved lexically (`~` and `~/` under the home directory, relative paths under the
  user's working directory, or the hidden shell's for a command, `.` and `..` folded),
  so the conversation can ask before anything runs. Its `Debug` shows the command's
  length, not its text.
- `ToolRegistry`: `register`, `specs` (for the provider request, in registration
  order), and `requirements`, `preview`, `takes_manual_input` and `invoke` by name. It
  never hands out a tool.
- `Tool::takes_manual_input(input)`: whether a call takes an input that the user
  chooses to type while it reports no wait (false by default). The shell tool takes one
  unless its line goes to a nested shell or starts a shell or a REPL
  (`efr_shell::takes_manual_answers`), where the hidden shell refuses it.
- `write_file` previews a write as a unified diff against the current file (every
  line added for a new file), at most 200 lines and 16 KiB, with a line that says
  how much is left out. `unified_diff` is that diff, public so the daemon's settings
  tool shows a change of `config.toml` the same way. After a write, the result's
  `written` lists each file the call changed (`WrittenFile`): its path, what happened
  to it (`WrittenKind`: created, changed, deleted, or moved with the path it came
  from) and `written_diff`'s diff of the change (at most `MAX_CALL_DIFF_LINES` lines
  and 256 KiB, then `... N more lines`) with the counts of added and removed lines; a
  file that held bytes that are not UTF-8 text is `binary` and gets no diff.
- `ToolContext`: the call's ids (`CallIds`), the user's working directory, where the
  hidden shell is now (`shell_cwd`, when one runs), `$SCRATCH`, the scope, the origin,
  the home directory (`efr_scope::Home`), the clock, the write journal,
  `forget_credentials`, which the shell tool passes on to the run so the hidden shell
  forgets sudo's credentials after the call (`shell.sudo_cache = "per_call"`), and
  `interactive_limit`, set for a call that the user approved because it may wait for
  input (`shell.interactive_timeout_minutes`), which the shell tool passes on as
  `RunRequest::interactive_limit`, and `sandbox` (`efr_shell::SandboxRun`), set for
  a shell call of the `auto` mode that the daemon prepared for the sandbox's launcher,
  which the shell tool passes on as `RunRequest::sandbox`.
- `ToolResult`: the output the model sees, the truncation flag, the error flag, the
  exit code, and for a call through the sandbox's launcher its summary (names only)
  and whether the sandbox could not start, so the daemon checks the sandbox again; `ToolOutputSink` hears a call's output while it runs, each change of
  whether its command waits for input with whether a visible prompt looks like a
  password prompt behind a relay (`input_changed`, ignored by default), and is asked
  whether a person can answer hidden input now (`can_answer_hidden`, true by default)
  and whether a person who can answer follows the call (`can_answer`, false by
  default, which keeps a call's timeout); the daemon answers both.
- `freeform_text(input)`: the text of a freeform tool's call. A model that takes
  freeform tools writes the text itself, which arrives as a JSON string; any other
  model calls the function form, `{"input": "<text>"}`. A freeform tool reads its
  input only through this function, so both forms work the same. The member name
  must equal `efr_provider::FREEFORM_INPUT`; a test in `efr-daemon` checks it.
- `truncate_middle`: the head and the tail of a long output with a
  `[... N bytes omitted ...]` line between them, cut on character boundaries and near
  line ends; `DEFAULT_OUTPUT_LIMIT` is 32 KiB.

The tools:

- `ShellTool` (`shell`) runs a command line in the conversation's hidden zsh through
  `efr_shell::CommandRunner` (the daemon passes its `ShellSessions`). Input:
  `command`, `timeout_seconds` (default 30, at most 600), `nested_shell` (sentinel
  mode, for a `sudo -i`, `bash` or `ssh` started inside the hidden shell; the `auto`
  mode refuses it) and `needs` (what a command needs beyond the `auto` sandbox, with
  the limits of `Needs::check`; a call over them is `ToolError::InvalidNeeds`). After
  a run through the sandbox's launcher, the answer names what `result.json` reports:
  the sandbox note after a contained command that failed, the exports that stay in the
  sandbox or were dropped, the background jobs that stopped, the hosts that the proxy
  refused, the git settings that the launcher moved away, a shell directory that the
  sandbox hides and a state that was not kept. A sandbox that could not start says
  why (`Completion::SandboxFailed`, from `setup_error` only), and a contained command
  that asked for a secret is told to ask with `needs.outside`. Names only, never values;
  efr never reads the output for a denial, because the command wrote it. A new shell
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
  the text cannot show which one `-t` takes, when an option follows an operand or when
  an option is unknown; never the value of `-S` or `--suffix`; the sources of a hard link, `ln` without
  `-s` or `cp -l`), `reads.rs` does the same for `git rm`, `git mv` and
  `git worktree add`, and `shell_tool/declare.rs` resolves them against the hidden shell's directory (with
  `$HOME` at the start of a word read as `~`, and `rev:path` also naming `path`), follows
  `cd` within the line, counts a path at or below what an earlier `cp`, `ln` or `mv`
  of the line wrote as anywhere below `/` too (it may be a link by then), turns a glob into everything below its fixed directory and an
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
  tool passes the run's input waits and the questions who can answer to its
  `ToolOutputSink`. The texts the model reads never promise a screen: the user does
  not see the hidden shell, and can answer a waiting command in their terminal only
  while they follow the turn; behind a relay (`sudo`, `ssh`, `docker exec`) the program
  on the inner terminal decides whether an answer is shown; an approved interactive
  call may run up to the user's interactive limit, and the timeout text names how long
  the call really ran; no editor works in the hidden shell. A full-screen program at the timeout
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

- `ApplyPatchTool` (`apply_patch`) edits files with a patch: the section "The
  apply_patch tool" below.

The file tools refuse a path that goes through a symbolic link, and name the real
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

## The apply_patch tool

`ApplyPatchTool` edits files with a patch in the `apply_patch` format of Codex, so
the model stops rewriting whole files or using `sed -i`. The engine is `efr-patch`;
this tool does the IO around it (`src/apply_patch.rs`). `write_file` stays.

Definition:

- Name: `apply_patch`.
- Spec: `ToolSpec::freeform("apply_patch", DESCRIPTION,
  ToolGrammar::Lark(efr_patch::GRAMMAR.to_owned()))`. The grammar lives in
  `efr-patch`, next to the parser that reads it.
- A provider sends the freeform form (a Responses `custom` tool with the grammar) to
  a model that takes it, and the function form (one string member `input`) to every
  other model. Which models take which form comes from the model catalog
  (`efr-provider-openai/src/catalog.rs`, `ModelInfo::freeform_tools`): a model whose
  `apply_patch_tool_type` is `freeform` takes the freeform form, as every model of
  Codex's catalog does today.
- The description tells the model the format with a short example, says that the
  patch is plain text and not JSON (in the function form, the whole patch goes in
  `input`), and says that paths are relative to the working directory or absolute.
- The input is `freeform_text(input)`. Any other input is `ToolError::InvalidInput`.
- The daemon registers the tool after `write_file`. `efr_config::DEFAULT_SYSTEM_PROMPT`
  (`efr-config/src/tables.rs`) tells the model to edit files with `apply_patch`, and
  never with `sed -i`, `perl -pi` or a `write_file` of the whole file for a small
  change.

Requirements (pure, like every `requirements`):

- The tool parses the patch with `efr_patch::parse`. A parse error is the `Err`
  that the model reads (`ToolError::Patch`): the line and the problem. An input that
  is not text is `ToolError::InvalidInput`.
- It resolves each path lexically as `write_file` does (`paths::resolve`: `~`,
  relative to the user's working directory, `.` and `..` folded), with
  `Patch::map_paths`.
- It declares every path it touches as a write (`AccessMode::Write`): the file of
  an update, of an add and of a delete, and both the source and the target of a
  move. So in a registered project an update or an add is routine exactly where a
  `write_file` of the same path is routine today.
- A delete or a move (`Operation::is_destructive`) also sets
  `ToolRequirements::destructive`. The daemon copies it to
  `efr_permissions::Requirements::destructive`, and the engine answers `Ask` for it
  in every mode, `auto` too, and from every origin, whatever the rules say, until
  undo exists (phase 4 of the auto spec).

Preview (the approval card): the unified diff of every file of the patch, none left
out, each file within the bounds of `written_diff` (`MAX_CALL_DIFF_LINES` lines and
256 KiB), with absolute paths as `write_file`'s preview has them. A delete and a move
have a header line before their diff that says so, such as
`delete /home/u/p/old.rs` and `move /home/u/p/a.rs -> /home/u/p/b.rs`, so the card
can mark them. The daemon shows these paths as the diff of the finished call does
(relative to the project, `$SCRATCH/` or `~/`). After 4 MiB of preview, the files that follow are only counted
(`[... N more files of the patch]`), so the event stays far below the frame limit. A
patch that does not apply has no preview; its call fails when it runs.

Invoke, all or nothing:

1. Read every path of the patch. Refuse, as `read_file` and `write_file` do, a path
   through a symbolic link (`paths::check_real`, naming the real path), anything
   that is not a regular file, a file over 16 MiB and a binary file (a NUL byte in
   the first 8 KiB). A path with no file is absent from the `efr_patch::Files`.
2. Call `efr_patch::apply`. An error changes nothing.
3. For each `FileChange`, in order: record the original of every path it changes
   (both paths of a move) in the context's `WriteJournal` as `write_file` does, and
   write nothing when the journal fails; then write. An add or an update writes
   atomically with `efr_stdx::fs::write_atomic`, keeps the mode, the owner and the
   group of an existing file, gives a new file mode 0644 and creates missing parent
   directories. A content that a move brought from another path
   (`FileChange::from`, as in a chain of moves) keeps the mode and the owner of the
   file it came from. A delete removes the file. A move writes the target, then
   removes the source.
4. When a journal entry or a write fails, restore every file that the call already
   changed from the snapshots that the tool took in step 1 (the same originals that
   went to the journal), in reverse order, remove the empty directories that the call
   made, and fail with the error. The result says that no file was changed, or names
   a file that could not be restored.
5. Steps 3 and 4 run in a task of their own (`tokio::spawn`), which the call only
   waits for. When the user interrupts the turn, the turn drops the call's future;
   a guard then tells the task to stop. The task stops before its next change and
   restores every file as in step 4, so an interrupt never leaves a patch half
   applied.

Result:

- Success, short: `Success. Updated: a.rs, b.rs; Added: c.rs; Deleted: d.rs;
  Moved: e.rs -> f.rs`, with each group left out when empty and the paths as the
  patch wrote them. A content that a move brought from another path names it, such
  as `Added: b.sh (from a.sh)`.
- Failure: the error in one sentence, then for `PatchError::NoMatch` and
  `PatchError::NoAnchor` the hunk's anchors and the nearest lines of the file with
  their numbers, for `PatchError::Ambiguous` the line numbers of the matches and a
  request for more context or an `@@` line, then `No file was changed.`, so the
  model can correct the patch and try again. An add of a file that exists is
  `PatchError::Exists`: the model updates the file instead, or deletes it first.
- `ToolResult::written` lists every file that the call changed, in patch order: a
  `WrittenFile` whose `WrittenKind` says whether the file was created, changed,
  deleted or moved (and from where). Their diffs together stay within
  `MAX_CALL_DIFF_LINES` lines: a file after the limit has its header and
  `... N more lines`. The daemon turns the list into the `changes` and the `diff` of
  `tool_call_completed`, the diff of every file in patch order, one after the other,
  so the CLI shows one diff block per file.

## Tier

Tier 2. The only edge inside the tier is `efr-tools -> efr-shell`.

## Allowed dependencies

`efr-shell` (the `CommandRunner` trait and its request and result types),
`efr-scope` (`Home`, the home directory in both forms), `efr-patch` (the patch
engine of `apply_patch`), `efr-protocol` (ids, `Scope`, `Origin`) and `efr-stdx`
(`Clock`, atomic writes). `xtask/src/deps.rs` holds the
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
with symbolic links made where a test needs one). The tests of `apply_patch` run on
the engine of `efr-patch`; the shell tool runs against a fake
`CommandRunner` that scripts results and progress, and tables cover the split, the
paths each program reads and the declared paths with `cd` and globs. No test starts a
shell, uses the network or touches the user's home.

One test reads a file of another crate on purpose:
`src/shell_tool/tests/corpus.rs` compiles in `efr-permissions`'
`tests/fixtures/auto-corpus.toml`, the `auto` corpus, and checks that the shell tool
declares for each line what that fixture says it declares. The engine's corpus test
then judges what the daemon really passes it. The test reads only the keys of the
declaration, so a change to the engine's other keys does not break it. The code of
this crate still does not depend on `efr-permissions`; the test needs that crate's
directory in the checkout.
