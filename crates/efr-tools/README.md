# efr-tools

## Purpose

The tools the model calls, and the registry that offers them: `shell`,
`read_file`, `write_file`, `apply_patch` and `edit`. A request offers a model one of
the two edit tools, `apply_patch` or `edit`, never both: the daemon picks the one that
the model's `efr_provider::ModelInfo::edit_tool` names.

- `Tool`: `spec()` (a `ToolSpec`: name, description and the input's JSON Schema,
  generated with schemars from the input type, without `$schema` and `title`; or,
  from `ToolSpec::freeform`, a freeform tool whose input is plain text that a
  `ToolGrammar` describes, with the schema of its function form, one string member
  `FREEFORM_INPUT` (`input`)),
  `requirements(ctx, input)`, `preview(ctx, input)` (what a call would change, for
  its approval; none by default) and `invoke(ctx, input, out)`.
- `ToolRequirements`: every path a call touches with its `AccessMode` (read, read with
  everything below, or write), the command line it runs, whether it talks to the
  network or may wait for input at the terminal (`interactive`, the hard flag),
  whether it may ask for a password through a program that the hard flag does not
  cover (`may_prompt`, the soft flag), whether it deletes or moves a file
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
  (`efr_shell::takes_manual_answers`), where the hidden shell refuses it, or the call
  starts a background job, whose stdin is `/dev/null`.
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
  hidden shell is now (`shell_cwd`, when one runs), `$SCRATCH` (which the shell tool
  puts on the run, so the hidden shell exports it as `SCRATCH`), the scope, the origin,
  the home directory (`efr_scope::Home`), the clock, the write journal,
  `forget_credentials`, which the shell tool passes on to the run so the hidden shell
  forgets sudo's credentials after the call (`shell.sudo_cache = "per_call"`, and
  every call of full mode), `forget_before`, which the shell tool passes on as
  `RunRequest::forget_before` so the shell forgets them before the line too (full
  mode; a shell without efr's zsh integration fails the call with a text that says
  that full mode needs it, and the command does not run), and
  `interactive_limit`, set for a call outside the sandbox whose line may ask at the
  terminal, whoever approved it (`shell.interactive_timeout_minutes`), which the shell
  tool passes on as `RunRequest::interactive_limit`, `input_limit`, the same limit for
  any command while it waits for input, which it passes on as
  `RunRequest::input_limit`, `approved_by`, who approved the call, which it passes on
  with the line's hard or soft flag as `RunRequest::with_approval`, and `sandbox` (`efr_shell::SandboxRun`), set for
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
  mode refuses it), `needs` (what a command needs beyond the `auto` sandbox, with
  the limits of `Needs::check`; a call over them is `ToolError::InvalidNeeds`; the text
  of `needs.hosts` sends a server that the user's browser must reach to a background
  job, whose port efr forwards, and keeps `localhost` for a tool that does not use
  `HTTPS_PROXY`, such as psql, but tells the model not to name the forwarded port of a
  job, which a contained call reaches on 127.0.0.1), and
  `background`, `max_minutes` and `watch` (phase C: a background job, which efrd starts
  and which runs on after the call answers; `job_request` reads them, and a call that
  breaks their rules is `ToolError::InvalidJob`). The
  description has one text for every mode. Its `auto` sentences tell the model to run
  what the user asked without asking for permission first, to call again with the same
  line and the narrowest `needs` after an access error, and that efr decides what
  leaves the sandbox. Its job sentences say that `background: true` answers at once
  with a job id and an output file, that the job runs until it ends, `max_minutes`
  passes or `job_stop` stops it, that a notice comes at its end so the model does not
  poll, and that the `&`, `nohup` and `setsid` processes of a normal call stop when the
  call ends. `job_moved_text` (from v0.0.4) is the answer of a call whose running
  command the user moved into a job with Ctrl+B: the job id, the output file, the time
  limit from the move, no poll, the new hidden shell without the old variables,
  functions and aliases, and for a contained command that it gets no port forwards and
  no longer reaches the forwarded ports of other jobs. efrd writes it after the output
  so far. After
  a run through the sandbox's launcher, the answer names what `result.json` reports:
  the sandbox note after a contained command that failed (where it writes and that
  secrets read as empty, never a list of what it cannot do), the exports that stay in the
  sandbox or were dropped, the background processes that stopped (with the advice to
  start such a process with `background: true`), the hosts that the proxy
  refused (`blocked_note`: one sentence per reason, with the hosts that got it, and one
  for the refusals that the list leaves out; for `denied` it says that no host grant
  helps; efrd uses it for a job's end too), the bus
  calls that efr's bus filter refused (`bus_note`: each refused call as data, with its
  bus, name, member and path, how to ask for the whole bus with `needs.bus`, and one
  line when the filter stopped during the command; efrd adds it after the filter
  ended, for a call and for a job's end), the git
  settings that the launcher moved away, a shell directory that the
  sandbox hides and a state that was not kept. A sandbox that could not start says
  why (`Completion::SandboxFailed`, from `setup_error` only), and a contained command
  that asked for a secret is told to ask with `needs.outside`. Names only, never values;
  efr never reads the output for a denial, because the command wrote it. A new shell
  starts in the user's working directory. It declares the command line, `interactive`
  when a program of the line may wait for input (`sudo`, `ssh`, an editor, a pager) or
  the call targets a nested shell, `may_prompt` when the line may ask for a password
  through ssh (`shell_tool/prompts.rs`: `rsync` with a remote operand or a remote
  shell in `-e`, `--rsh` or `RSYNC_RSH`; `scp`, `sftp`, `sshfs`, `ssh-copy-id`,
  `mosh`, `autossh`, `ssh-add`, `ssh-keygen` without `-N`; `git` over the network,
  past every git option that takes a value, also `remote update`, `archive --remote`
  and `send-email`; `git` with `GIT_SSH_COMMAND`, `GIT_SSH` or `core.sshCommand`; a
  remote shell counts when it runs `ssh` or a program of the hard flag, and never
  gives the hard flag), and `network` when a program usually reaches the
  network (`curl`, package installs and syncs but not `pacman -Q` or `-Ss`, nor the
  scripts of `npm run`, `npm test` and the like, `git pull`); all three are a
  heuristic over the line's words, and the engine judges the command line itself
  too. The soft flag refuses no job. It declares the directory the line starts in (the hidden
  shell's, or the user's for a new shell), so a rule may allow a command in one
  project only. It also declares the paths the line names, so the path
  rules judge what a freely allowed read-only command reads: `shell_tool/words.rs`
  splits the line (quotes read as zsh reads them, the commands inside `$(...)` and
  groups included) and never fails, `shell_tool/reads.rs` says which words of each
  program are paths (every operand and every path-like option value, also the value
  of a separate `-o Name=value` of `ssh`, `scp`, `sftp`, `autossh`, `sshfs` and `mosh`
  when it starts with `~/`, `/` or `.`, so `-o ControlPath=~/.ssh/cm` declares
  `~/.ssh/cm` as `-oControlPath=` does; nothing for
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
  `cd` within the line (past 1024 directories that the rest of the line may run in, a
  relative path counts as anywhere below `/`, as after `cd -`), counts a path at or below what an earlier `cp`, `ln` or `mv`
  of the line wrote as anywhere below `/` too (it may be a link by then), turns a glob into everything below its fixed directory and an
  output redirection into a write. So `cat ~/.ssh/id_ed25519` declares the key and is
  denied, `rg TOKEN ~/.aws` declares `~/.aws` with everything below and asks, and
  `cp x ~/.config/efr/config.toml` declares a write of efr's config, which every mode
  denies. The engine's `auto` mode lets a writer program run because its writes are
  declared: the path rules decide them.
  What the text cannot show, such as the files a script opens, it cannot declare. The
  answer
  ends with `[exit code N, cwd DIR]`; a command still running at the timeout gets the
  screen's last lines and a note that it holds the shell, and the result has
  `ToolResult::left_running` (`Completion::Interactive`, `FullScreen` and
  `StillRunning`), which the conversation records on `tool_call_completed`. The run
  names the call (`RunRequest::call`), so only answers for this call reach its command,
  and the tool passes the run's input waits and the questions who can answer to its
  `ToolOutputSink`. The texts the model reads never promise a screen: the user does
  not see the hidden shell, and answers a waiting command in efr's view while they
  follow the turn, or after `,shell`; a text never tells the model to have the user
  type at their own prompt; behind a relay (`sudo`, `ssh`, `docker exec`) the program
  on the inner terminal decides whether an answer is shown; a call outside the sandbox
  whose line may ask, and any command that waits for input, may run up to the user's
  interactive limit, and the timeout text names how long the call really ran; no
  editor works in the hidden shell. The description also tells the model that `rsync`,
  `scp` and `git` over ssh may ask for a password, that a command left running holds
  the shell, and not to start an ssh control socket, because `~/.ssh` is a secret path.
  A full-screen program at the timeout (`Completion::FullScreen`) has its own text:
  the user is never asked about one and can only stop it with `,shell`, so the model
  reads that, not that the user missed a question. A command that waited for hidden
  input that nobody could answer (`Completion::Unanswered`) is an error result that
  says efr interrupted it and that the user should run it in their own terminal or
  follow the turn while the model tries again. A busy shell, a shell that did not
  reach its prompt, a shell that exited and a command that cannot be typed are error
  results with advice for the model; other shell failures are `ToolError::Shell`. The
  advice is only what is true: nothing clears a busy shell from a turn (an interrupt
  reaches the shell only for a call in flight), so the model is told to tell the user,
  and that a new conversation gets a fresh shell. `not_ready_text` (with `NotReady`)
  says what holds the hidden shell: no prompt yet in a new shell (`Fresh`), an earlier
  command that the call cannot name (`Held`), a command left running (`Running`, with
  how long it ran), or one that waits for input (`Waiting`, from
  `ShellError::HeldBy`): the call fails at once, the model learns that the user answers
  or stops it with `,shell`, and it must not call again or ask the user to type at
  their prompt. Only outside `auto` and only for a held shell or REPL does the text
  name `nested_shell`.
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
- `EditTool` (`edit`) replaces exact text in one file: the section "The edit tool"
  below.

The file tools refuse a path that goes through a symbolic link, and name the real
path in the error, because the permission engine judged the path as written: a
second call with the real path is judged on its own. A home reached through a link
(`/home` to `/var/home`) is not refused, since the engine knows both forms of it.

A shell command cannot be refused that way, because `/bin`, `/etc/resolv.conf` and
many other paths are links. `ToolRequirements::with_real_paths` adds, after each
declared path that reaches the file system through a link, the path it reaches with
the same access, so `cat notes`, where `notes` links to `~/.ssh/id_ed25519`, is judged
as a read of the key too, and a recursive search whose root is a link is judged by its
target as well. It keeps each link with the path it reaches in
`ToolRequirements::links`, so the daemon can tell the engine that the real file of
`~/.zshrc` is named by the names of `~/.zshrc`. It is the one step that reads the file
system (it blocks; the daemon runs it on the blocking pool before the engine decides). A link that a glob expands
to, or one below the root of a recursive search that the program follows, is not
seen. It looks up each directory once for the whole call (`paths::RealForms`), and a
path below a directory that does not exist, or that cannot be searched, needs no
look-up of its own, because the file system resolves a path through its parents
first: the `cd`s of one line declare thousands of paths below a few dirs that do not
exist.

## The apply_patch tool

`ApplyPatchTool` edits files with a patch in the `apply_patch` format of Codex, so
the model stops rewriting whole files or using `sed -i`. The engine is the module
`patch` (see "The patch engine" below); this tool does the IO around it (`src/apply_patch.rs`). `write_file` stays.

Definition:

- Name: `apply_patch`.
- Spec: `ToolSpec::freeform("apply_patch", DESCRIPTION,
  ToolGrammar::Lark(patch::GRAMMAR.to_owned()))`. The grammar lives in the
  `patch` module, next to the parser that reads it.
- A provider sends the freeform form (a Responses `custom` tool with the grammar) to
  a model that takes it, and the function form (one string member `input`) to every
  other model. Which models take which form comes from the model catalog
  (`efr-providers/src/openai/catalog.rs`, `ModelInfo::freeform_tools`): a model whose
  `apply_patch_tool_type` is `freeform` takes the freeform form, as every model of
  Codex's catalog does today.
- The description tells the model the format with a short example, says that the
  patch is plain text and not JSON (in the function form, the whole patch goes in
  `input`), and says that paths are relative to the working directory or absolute.
- The input is `freeform_text(input)`. Any other input is `ToolError::InvalidInput`.
- The daemon registers the tool after `write_file`, and `edit` after it.
  `efr_config::DEFAULT_SYSTEM_PROMPT` (`efr-config/src/tables.rs`) tells the model to
  edit files with the edit tool that it has (`apply_patch` or `edit`), and never with
  `sed -i`, `perl -pi` or a `write_file` of the whole file for a small change. The
  description of each edit tool says the same.

Requirements (pure, like every `requirements`):

- The tool parses the patch with `patch::parse`. A parse error is the `Err`
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
   the first 8 KiB). A path with no file is absent from the `patch::Files`.
2. Call `patch::apply`. An error changes nothing.
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

## The edit tool

`EditTool` changes one file with an old and a new string, in the shape of Claude
Code's edit tool, so a Claude model edits files in the form that it knows. The engine
is `patch::replace`; this tool does the IO around it (`src/edit.rs`).

Definition:

- Name: `edit`.
- Input (a function tool, `ToolSpec::for_input`): `path`, `old_string`, `new_string`
  and `replace_all` (a boolean, false by default). Unknown members are refused.
- The description says to use the tool for each change to a file and not `sed -i`,
  `perl -pi` or a `write_file` of the whole file, that `old_string` is the exact text
  of the file and must occur once unless `replace_all` is true, and that a new file
  is made with `write_file`. It does not say that the model must read the file first,
  because the tool does not check it.

Requirements: the path, resolved as `write_file` resolves it, as a write. The tool is
never destructive: it changes one file that exists and never makes, deletes or moves
one. So its approvals follow `write_file`.

Invoke:

1. An empty `old_string` and the same text in `old_string` and `new_string` are error
   results before any file is read.
2. Read the file with the checks of `apply_patch`: a path through a symbolic link,
   anything that is not a regular file, a file over 16 MiB and a binary file are
   refused. A path with no file is an error result that says to use `write_file`.
3. Line ends: when every line of the file ends with `\r\n` and neither string holds a
   `\r`, each `\n` of both strings becomes `\r\n` before the match, so the exact
   match works and the new lines keep the file's line ends. A file with mixed line
   ends takes the strings as written.
4. `patch::replace` with `Occurrences::One`, or `Occurrences::All` for
   `replace_all`.
5. Record the original in the context's `WriteJournal`, and write nothing when the
   journal fails; then write atomically with the mode, the owner and the group of the
   file. An interrupt drops the call's future before the write, or after it; never in
   the middle.

Result:

- Success: `Success. Replaced 1 match in src/a.rs.` (`N matches` with `replace_all`),
  with the path as the model wrote it. `ToolResult::written` holds the file as
  `WrittenKind::Changed` with its diff.
- Failure, as error results that the model reads:
  - several matches: `Found 3 matches of old_string, but replace_all is false. Give
    more context to make one match, or set replace_all to true.`
  - no match: `old_string was not found in the file.`, the nearest lines of the file
    with their numbers (as `apply_patch` shows them), and a request to read the file
    again.
  - `old_string is empty. Use write_file to make a new file.`
  - `No change: old_string and new_string are the same.`

Preview (the approval card): the diff of the file in the form of one file of
`apply_patch`'s preview (`written_diff`, at most `MAX_CALL_DIFF_LINES` lines), with
the absolute path. The daemon shows the path as it does for `apply_patch`. A call that
would fail has no preview.

## The patch engine

The module `efr_tools::patch` (`src/patch.rs` and `src/patch/`) is the patch engine.
It reads the text of an `apply_patch` call and computes the new content of each file. It does no IO: the caller reads the files, gives their text to
the engine, and writes the result. The same engine serves the `apply_patch` tool and
the `edit` tool with an old and a new string (`replace`), which Claude models get.

The format is the `apply_patch` format of Codex. `GRAMMAR` is its Lark grammar,
written for efr: it describes the same language as
`codex-rs/core/assets/tools/apply_patch.lark`, so a model that was trained on that
format writes text that it accepts. A provider sends the grammar with the freeform
`apply_patch` tool.

```text
*** Begin Patch
*** Add File: docs/new.md
+# New
*** Delete File: old.txt
*** Update File: src/lib.rs
*** Move to: src/core.rs
@@ impl Engine
@@     fn run(&self) {
-        old();
+        new();
         done();
*** End of File
*** End Patch
```

### API contract

The entry points are pure functions. The tools build on these types and signatures. A
change to them is a change of this contract.

- `parse(text: &str) -> Result<Patch, PatchError>`. A `Patch` holds its
  `operations` in the order of the text. An `Operation` is one of:
  - `Add { path, content }`: the text of the `+` lines, each line with its newline.
  - `Delete { path }`.
  - `Update { path, move_to, hunks }`: `move_to` is the target of
    `*** Move to:`. A `Hunk` has its `anchors` (the text of each `@@ <text>` line
    before it, in order; a bare `@@` adds none), its `lines` (`HunkLine::Context`,
    `Remove` and `Add`, without the marker and the newline) and `end_of_file` (after
    `*** End of File`).

  Paths stay as the text writes them. The caller resolves them with
  `Patch::map_paths`. `Patch::paths` gives each path once, in order, with the target
  of a move after its source. `Operation::is_destructive` is true for a delete and
  a move. Text that is not a patch is `PatchError::Parse { line, problem }`, with
  the line from 1 and a `ParseProblem`.

- `apply(patch: &Patch, files: &dyn Files) -> Result<Vec<FileChange>, PatchError>`.
  `Files::text(path)` gives the current text of a path, or `None` when no file is
  there. `HashMap<PathBuf, String>` and `BTreeMap<PathBuf, String>` implement it.
  The rules:
  - The engine computes every new content before it returns. It writes nothing, so
    the caller can write all changes or none.
  - The operations apply in order. Each operation sees the result of the operations
    before it: a file that an earlier operation removed is absent, and a file that
    it added is present.
  - The result has one `FileChange { path, kind, from }` for each path that changes, in
    the order that the patch first names the path. A later operation on the same
    path folds into that change. `ChangeKind` is `Added { content }`,
    `Updated { content }`, `Deleted` or `Moved { to, content }`. A path whose text
    ends as it started has no change. `Moved` is for a file that leaves a path that
    had a file for a path that had none. Any other result of a move shows as a
    delete and an add or an update. Then `from` of the add or the update names the
    path that the file had before the patch, so the caller keeps that file's mode.
    It is `None` for a new file, a file that stays at its path, a delete and a
    `Moved`.
  - An add of a file that exists, and a move onto a file that exists, are
    `PatchError::Exists { path }`. An update, a delete or a move of a file that does
    not exist is `PatchError::Missing { path }`. A move to its own path is an update.
  - The hunks of one update apply from the top of the file down. The search for a
    hunk starts after the hunk before it.
  - Each `@@ <text>` anchor names the first line after the anchor before it (after
    the previous hunk for the first anchor) that it matches. An anchor matches a
    line in the passes below, and then in one more pass: the line starts with the
    anchor, and the anchor does not end inside a word (a letter, a digit or `_`
    after it). So `@@ fn run()` names the line `fn run() {`, but `@@ impl Foo` does
    not name `impl FooBar {` and `@@ fn run` does not name `fn run_all() {`. A
    missing anchor is
    `PatchError::NoAnchor { path, hunk, anchor, nearest }`.
  - The old lines of a hunk (its context and `-` lines) match in passes. The first
    pass is exact. Then the engine ignores trailing whitespace. Then it ignores all
    whitespace around each line. Then it also reads Unicode punctuation (dashes,
    quotes, special spaces) as ASCII. The first pass that finds a match decides.
  - With an anchor, the search starts at the line of the last anchor, and the first
    match wins. Without an anchor, the match must be the only one after the
    previous hunk. Else the hunk is `PatchError::Ambiguous { path, hunk, lines }`,
    with the first line of each match, so the model can add context or an anchor.
  - A hunk with `end_of_file` must match at the end of the file.
  - A hunk that has no old lines inserts its lines after its last anchor. Without an
    anchor, or with `end_of_file`, it appends them to the file, as in Codex. Such a
    hunk does not move the start of the search for the next hunk.
  - When no pass finds a hunk and its last old line is empty, the engine tries once
    more without that line. Models often write the final newline of a file as an
    empty context line. A hunk whose only old line is that empty line is not tried
    again, because without it the hunk would append to the file.
  - A hunk that matches nowhere is `PatchError::NoMatch { path, hunk, nearest }`.
    `hunk` counts from 1 in its update. `nearest` holds at most 12 `NearLine
    { number, text }` of the file: the place that shares the most lines with the
    hunk (else the line most like its longest line), with one line before and after.
  - Context lines keep the text and the line ending that they have in the file, so a
    tolerant match changes only the `-` and `+` lines. A new line takes the first
    line ending of the file (`\n` when the file has none). The new content keeps the
    final newline of the old file, or the lack of it. An empty file counts as
    ending in a newline.

- `replace(text, old, new, occurrences: Occurrences) -> Result<Replacement,
  PatchError>`, for the edit tool. The match is exact, with no tolerant pass.
  `Occurrences::One` needs exactly one occurrence, else
  `PatchError::NotUnique { count }`. `Occurrences::All` replaces every
  occurrence. An `old` that does not occur is `PatchError::NotFound { nearest }`,
  with the lines nearest to `old` as for `NoMatch`, and an empty `old` is
  `PatchError::EmptyOld`. `Replacement` holds the new `text` and the `count` of the
  replacements.

`PatchError` is the one error type of the engine. Its `Display` text is one sentence.
The tools write the longer text that the model reads from the fields.

### What the parser accepts

The parser is strict on structure: each problem is a `ParseProblem` at its line.
It accepts these variants, as the parser of Codex does:

- Blank lines before `*** Begin Patch` and after `*** End Patch`.
- Whitespace around `*** Begin Patch`, `*** End Patch` and the operation lines
  (`*** Add File:` and the others). Inside an update, only trailing whitespace: a
  line that starts with a space is a context line, even when it looks like a
  marker.
- `\r\n` line ends in the patch, and no newline after `*** End Patch`.
- A patch in a heredoc: a first line `<<EOF`, `<<'EOF'` or `<<"EOF"`, and a last line
  that ends with `EOF`.
- An empty line in a hunk, read as an empty context line.
- No `@@` before the first hunk of an update, and no `*** End of File`.
- Blank lines after `*** End of File`.
- An add with no `+` line: an empty file.

It also accepts these variants, which Codex refuses:

- Several `@@` lines in a row. Each one narrows the search for the next, such as
  `@@ impl Engine` and then `@@ fn run(&self) {`.
- A unified diff header as an anchor: `@@ -12,7 +12,8 @@ fn run()` reads as
  `@@ fn run()`, and `@@ -12,7 +12,8 @@` as a bare `@@`.
- An update with a move and no hunk: a rename.
- Blank lines between operations, except in an added file, where every line must
  start with `+`.
- No space between a marker and its path, and whitespace around the path.

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

The patch engine is pure. It reads no file, writes no file and has no runtime. A
patch applies completely or not at all: the engine returns every new content or an
error, and the tool writes only after a success. `cargo xtask tidy` keeps the module
`patch` pure: its files name no `tokio`, no `async`, no `std::fs`, `std::io`,
`std::net`, `std::process` or `std::env`, no workspace crate and no `crate::` path
outside `crate::patch` (`PURE_MODULES` in `xtask/src/tidy.rs`). It was a crate of its
own with a forbidden edge to tokio until ADR 0014 folded it into this crate.

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
with symbolic links made where a test needs one). The tests of `apply_patch` and
`edit` run on the real engine; the shell tool runs against a fake
`CommandRunner` that scripts results and progress, and tables cover the split, the
paths each program reads and the declared paths with `cd` and globs. No test starts a
shell, uses the network or touches the user's home. The real forms of the declared
paths have property tests (`src/paths/tests.rs`): on a temporary tree with links to
dirs and files, relative, dangling and looping links, a link to `..` and a dir that
cannot be searched, `RealForms` and `with_real_paths` give what the look-up of each
level of each path gave in v0.0.4.

The engine has unit tests next to each module (`src/patch/<module>/tests.rs`). The
integration binary `tests/it` holds the property tests of `patch.rs`: a patch made
from a diff of two random texts turns the first text into the second. With repeated
lines, the patch gives the second text or `PatchError::Ambiguous`, never a wrong text.
The line diff has a property test of its own (`src/diff/tests.rs`): its hunks, applied
to the first of two random texts, give the second, with any work budget.
These tests use no file system, no network and no Zig; `cargo nextest run -p efr-tools
-E 'test(/^patch::/)'` runs only the engine's.

One test reads a file of another crate on purpose:
`src/shell_tool/tests/corpus.rs` compiles in `efr-permissions`'
`tests/fixtures/auto-corpus.toml`, the `auto` corpus, and checks that the shell tool
declares for each line what that fixture says it declares. The engine's corpus test
then judges what the daemon really passes it. The test reads only the keys of the
declaration, so a change to the engine's other keys does not break it. The code of
this crate still does not depend on `efr-permissions`; the test needs that crate's
directory in the checkout.

## Benches

Ignored tests that `just bench-perf` runs in release. They use the bench helpers of
`efr-test-support`, a dev-dependency only. They report what they measure, and a bench
with a budget fails when the budget fails. A size that takes more than 10 s stops its
ladder.

- `bench_perf_declare_cds` (`src/shell_tool/declare/tests.rs`): `declared` on
  `cd d1 && ... && cd dK && cat x` and on K groups `(cd gN && make)`, K = 5, 10, 15,
  20, 25, 50 and 100, with the count of the directories that `cat x` runs in. Each K
  runs in a child process (this test binary again, with `EFR_BENCH_DECLARE_CASE`),
  which the bench kills after 15 s. Budget: no K stops the ladder, `cat x` runs in at
  most 1024 directories, a line declares at most 4096 paths, each K takes less than
  10 ms, and the slope over the K past the cap (15 and more) stays below 1.3.
- `bench_perf_real_paths` (`src/paths/tests.rs`): `with_real_paths` on the paths that
  the shell tool declares for `cd c1 && ... && cd cK && cat x; cd <project>`, K = 4,
  6, 8 and 10, in a project where `c1/.../c10/x` exists, with the count of the
  declared paths and of the look-ups, and the time of the look-ups of v0.0.4 (one for
  each missing level of each path: 35 ms for the 10 cds). Budget: at 10 cds the
  look-ups stay below a tenth of the paths and the call takes less than 15 ms, and the
  slope over the paths stays below 1.3.
- `bench_perf_line_diff` (`tests/it/perf.rs`): `written_diff` on two one-line changes
  in files of 3k, 10k and 50k lines, and on a full rewrite of 50k lines, with the
  diff's line count and its added and removed counts. Budget: the two changes give
  two hunks (at most 20 lines with the header) and 2 lines added and 2 removed at
  each size, and the slope over the lines stays below 1.3.
- `bench_perf_patch_apply` (`tests/it/perf.rs`): `patch::apply` on a 5 MB file of 50k
  lines with 1, 10 and 100 hunks, whose context matches exactly, with trailing spaces
  and with ASCII quotes for typographic ones.
