# 0018: Moving a running call to the background

Status: accepted, 2026-10-21.

## Context

The user asked for a key in the turn view that moves the agent's running shell call
into a background job: the turn goes on at once, the model reads that the command
moved, and a job notice follows when it ends. Claude Code uses Ctrl+B for this. The
user's need is "to send any long running task to the background while I wait for it".

A background job of ADR 0010 starts from its own launcher (`efr-sbx run --job`), with
no terminal, in a session of its own, with a lifeline to efrd. A running call is
different. It runs in the conversation's hidden zsh, on the zsh's PTY, through the
launcher of a call (or with no launcher in `cautious`, `manual` and full mode). Its
sandbox, its proxy socket and its bus filters belong to the call. A process cannot
leave its PTY, its process tree or its sandbox without ptrace or a stop. And the
launcher of a call writes the shell state that it read at its start back at its end.

This choice is expensive to reverse: it gives a job a second kind of process, it keeps
two hidden shells of one conversation alive at one time, and it makes a key press of
the user the answer to a job question.

## Decision

1. **The job keeps the old shell.** The moved command keeps its PTY, its process tree,
   its sandbox and its grants. The job owns the old zsh and its PTY until the command
   ends; then efrd closes that zsh. The conversation's slot lets go of the old shell at
   the move, so the next call starts a new hidden shell in the old shell's directory.
   The new shell has none of the old shell's variables, functions or aliases. The
   sandbox state of the conversation (`state.json`, `state.zsh`) lives in the shell
   dir and stays.

2. **The hand-over in efr-shell.** `ShellSessions::request_move` takes a run that a
   tool call names while it waits for its command. efr-shell refuses when the command
   did not start, the run answered already, the shell gives no marks for the command
   (a sentinel run), the line starts a shell or a REPL, the command waits for input,
   or a full-screen program holds the screen. Else the session's actor keeps the run as
   its adopted run, and the job gets an `AdoptedRun`: the PTY id, the zsh's pid and
   start time, the output so far, the cleaned output after the move (a bounded queue
   that never holds up the shell, read by a streaming cleaner), the end, the input
   looks, `interrupt` and `close`. The call's run answers with `Completion::Moved`.
   The old shell's end is no `shell_exited`, and its PTY id no longer reaches a shell
   of the conversation.

3. **A second kind of job process.** `JobProcess::Launcher` is the job of ADR 0010.
   `JobProcess::Adopted` is a moved call, with its own rules:

   | Rule | A call through the launcher | A line typed into the hidden shell |
   |---|---|---|
   | Result | `result.json` in the call's dir | the exit status of the command's mark |
   | Lost launcher | no `result.json` | never |
   | Dirs that go at the end | the call's dir; the shell dir stays | none |
   | Stop | SIGINT to the shell's foreground job, then the close of the shell after 6 s | the same |
   | Sweep | every descendant of the zsh, then the zsh's session | the same |

   The job's proxy socket and bus filters move from the call's tool path into the
   job's entry. The reach of the call (ADR 0017) does not move: efrd sends `Unreach`
   for every port and drops it. The time limit is `jobs.default_minutes`, cut to
   `jobs.max_minutes`, from the move.

4. **The end of a moved launcher call.** After the hand-over, before the job is
   recorded, efrd makes the empty file `$CALL/moved` in the call's dir
   (`O_CREAT | O_EXCL`, 0600), which the sandbox cannot reach. A launcher that finds it
   at its end acts as a job's end: no `apply`, no `state.json`, no `state.zsh` and no
   config listings, but still `result.json`. Else a call that ends an hour later writes
   the shell state that it read at its start over the state of every later call. A
   launcher that ends before the hand-over keeps the state of a call that did not
   move.

5. **The key press is the answer, except outside the sandbox.** Ctrl+B sends
   `call.background` (scope `terminal`, so never the phone or a model-side peer), and
   it acts whatever the input row holds: the row gives up Ctrl+B as cursor-left. efrd
   checks the call (jobs on, a free place, no hard `interactive` flag, no nested
   shell, no cache layer lock of a contained call) and decides the line again as a
   job with the engine and the settings of the key press. Any `Deny` refuses, so work
   that no job may hold never becomes a job. A call in the sandbox then moves at once:
   it keeps its sandbox, so the move allows nothing new, and the key press is the
   user's answer, also in `cautious` and `manual`. A call outside the sandbox (an
   approved exit, `cautious`, `manual`, full mode) gets a preview with a token, the
   line, the limit and the exits of the job decision; the view asks one `y/n`
   question that says that the command keeps running with the user's full rights
   after the turn. Only a confirm with that token within 60 s and the same job
   decision moves it. The record keeps the exits that the `y` covered.

6. **Every descendant dies at the end.** Before each close of the old zsh, at a stop,
   at the end and after a wait for a secret, efrd kills every process below it: it
   walks `/proc` by parent from the zsh, which is a child subreaper, and kills each
   process through a pidfd that it checks again. So a process that left its session
   with `setsid` dies too. It must come before the close: once the zsh is gone, such a
   process is no longer below it. This matters most for a line typed into the hidden
   shell and for full mode, which have no launcher of their own.

7. **Nobody follows a job.** A moved command that asks for a password stops at once,
   and the notice says why. A plain question waits, and nobody can answer it: `efr
   jobs` shows that the job waits for input, and the notice of its stop says so.

8. **Undo.** The file changes that a moved command makes after the move join the turn
   of the call at the job's end, as for a job that the model started.

Rejected:

- Starting the command again as a job: it runs twice, and the first run's work is
  lost or doubled.
- ptrace (as `reptyr` does) to move the process tree to a new terminal: it needs
  `ptrace` rights over the command and into its sandbox, and it is fragile.
- Leaving the command in the shell and only telling the model: the shell stays busy,
  which is the incident of ADR 0017, and nothing ends the command.
- A question for every move: for a call in the sandbox, it asks about nothing new, and
  the user's need was one key.
- Ctrl+B only with an empty input row, so that it still moves the cursor back: a user
  who types a steer while the call runs could then not move it.

## Consequences

- Two hidden shells of one conversation live at one time. Code that names "the
  conversation's shell" reaches only the new one: the old one is out of the slot map,
  and its PTY id answers `not_found`.
- A moved call has no launcher time limit and no lifeline of its own. Only efrd's
  clock ends it at its time limit; if efrd hangs but lives, nothing ends it. The PTY
  is its lifeline: when efrd ends, the PTY closes and the shell ends. A PTY holder
  that outlives efrd (`efr-ptyd`, milestone 5) breaks this; that milestone must give
  a moved call a lifeline first.
- A moved call in the sandbox keeps writable git dirs and only the surface guard at
  its end. A job of its own has read-only git dirs and a guard every 5 minutes. The
  outside git check before each git line outside the sandbox still holds.
- A moved call gets no port forwards and loses its reach.
- In `cautious` and `manual`, the `y` to the move question is the answer to the job
  question of that mode.
- In tmux, Ctrl+B is tmux's own key, so the user presses it twice.
