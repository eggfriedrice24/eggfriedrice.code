# 0023: In auto, the reviewer decides each exit alone

Status: accepted, 2026-11-13.

## Context

ADR 0006 made `auto` a kernel sandbox with exits. In its first phase the user answered
every exit. Later phases added a reviewer model, which decided the exits that it was
sure about and handed the others to the user, and a class of exits that only the user
could approve, such as `sudo`, an upload that no message named and a read of a masked
file. efr's own rules also turned a reviewer's allow into a question when no message
of the user named the target or the host.

Daily use showed the cost of that design:

- Over 613 `auto` turns, 20 of each 100 turns had a question. The rate did not fall
  with time.
- In three weeks the user got 103 questions and said no to 3 of them.
- The reviewer decided 227 exits. It allowed 170, refused 1 and handed 56 to the user,
  who then allowed 43. In 17 of the 56, the reviewer's own category was "fits the
  request": the reviewer agreed, and a rule of efr asked.

So the questions were not a control. They were a habit of saying yes, and each one
stopped the work. Other agents of the same kind ask nothing in their automatic mode: a
classifier refuses an action, and the model tells the user.

Two facts of efr make the same step safer here than without a sandbox:

- The sandbox holds every command that stays inside it, with no review at all. Only
  an exit needs a decision.
- The reviewer reads the user's messages and facts that efr collects itself. It never
  reads a file, a web page or the output of a command. So text that the model read
  cannot reach the reviewer, except through the command line, which the reviewer's
  prompt marks as text of the model.

## Decision

1. While the reviewer is on (`classifier.enabled = true`, the default), efr asks the
   user nothing about an exit. The reviewer's answer is `allow` or `deny`. A refusal
   goes to the model with its reason, and the model tells the user. The user's next
   message is the way to ask for the action.
2. The class "only the user" becomes "the reviewer, under the request rule". The
   request rule: the reviewer's allow stands only when the reviewer also rates the
   user's authorization as `medium` or `high`. The same rule replaces the naming rule
   for a write or a host that no message names, and it holds for each high risk. A
   critical risk never allows.
3. Three exits of the highest risk run only by name (`Approver::Guarded`): a package
   publish, the whole session bus, and the socket of a credential agent that efr knows
   from the environment or that lies under a mask. efr refuses them with no review
   unless a message of the user names them, by a word match that efr does itself. The
   refusal shows the words. This is the one check that no model makes, so no text can
   talk its way past it.
4. Two exits stay the user's own question.
   - A write or a delete of a tree that the delete guard of full mode covers too (ADR
     0015), such as the home directory. A mode with a sandbox must not delete such a
     tree with less care than the mode without.
   - An exit that would run or install, outside the sandbox, code that a sandboxed
     command wrote or can write and that no reviewer reads (`ExitNeed::unread_code`):
     a script of the project in a line with `sudo` or in a line that runs outside the
     sandbox, a file from a write root that a line installs as a unit or an autostart
     entry, and a git hook that the sandbox can write. The reviewer reads the user's
     messages and the line, never the file, so its allow would give that code the
     user's full rights unseen. This is the reason of ADR 0006 for the sandbox: the
     model writes code freely in the project. An exit that runs this code outside the
     sandbox undoes what the sandbox is for. No model decides the mark. For a file,
     efr sets it from a fact of efrd that the file is there, in a write root. A first
     form of this rule read the options of the line (`-c`, `-e`) to tell inline code
     from a script. It was wrong on both sides: `bash -e ./deploy.sh` passed, and
     `bash -lc '...'` asked. So efr asks efrd about every word that can name a file,
     and no option of the line changes the answer. A relative word resolves in each
     directory where a `cd` of the line can lead, and the code of `sh -c '...'` is a
     line of its own, so `sudo sh -c './setup.sh'` gets the mark of its script. The
     same holds for a file that a line copies into a system directory, such as
     `sudo cp app.service /etc/systemd/system/`. Two cases have no file that the
     line names. A build or task tool, such as `make`, `just`, `npm` or `cargo`, runs
     what the files of its directory say: for a short list of such tools, the
     directory decides, and the mark comes when the tool can run in a write root. So
     `sudo make install` and `cargo test && git push` ask the user in a project. The
     cost is a question for a line that mixes a build with an exit; the model avoids
     it when it runs the build as its own line, in the sandbox. A package publish
     keeps decision 3. And where efr cannot tell what the line runs, such as
     `sudo "$TOOL"` or `sudo ./scripts/set*.sh`, the mark fails closed. Without the
     facts of efrd, which is a machine without the sandbox, there is no mark.
5. A refusal counts as a denial, whoever made it: the reviewer, a rule after a review,
   or a rule with no review. After 3 in a row, or 10 in one turn, the model can finish
   the work that stays in the sandbox, and its next exit stops the turn. An allow
   starts the count in a row again.
6. When no model of the reviewer's chain answers, the action does not run. That is no
   denial. After 3 such reviews in a row the turn stops, because the reviewer's model
   or the provider is down.
7. `classifier.enabled = false` keeps the old behaviour: the user answers every exit.
   This is no new key.

A question that is no exit stays: a rule of the user with the effect `ask`, a change
of efr's settings by the model, and a planted git setting that the surface guard
found.

## Consequences

- A wrong allow of the reviewer runs an action with no question before it. The
  protections that stay are the sandbox for everything that is no exit, the request
  rule, the three exits by name, the tree question, and `efr undo` for what efr copied
  first.
- The request rule rests on the reviewer's own rating of the user's authorization.
  Before, a name check that efr did itself stood behind each allow of an unnamed write
  or host. Now one loose rating of a small model can open such an action. Only the
  three exits by name keep a check that no model makes.
- The reviewer never reads a file. The second question of decision 4 covers the cases
  that a fact shows, and the build tools of a short list. It does not cover another
  program of the system that reads a file of the project, such as `docker compose up`
  with the project's compose file, or a file that the line hides, such as
  `sudo bash "$SCRIPT"`: that line goes to the reviewer under the request rule, and
  the reviewer's prompt tells it to refuse a line that hides the file it runs.
- efr cannot tell whether a `cd` works, so it counts the directory where the line
  starts too. `cd ~/src/tool && sudo make install`, typed in a project, asks a
  question that it does not need.
- A wrong refusal costs the user one message. That is more than one key, so the
  reviewer's prompt must not refuse what the user clearly asked for. The first live
  use must check this with real models: every test uses a recorded reviewer.
- No question means no `a` (always) answer. An always-allow rule is now a hand edit of
  `[[sandbox.allow]]` in `config.toml`. A command that adds a rule is open work.
- The engine keeps its classes. `Approver` gets one new variant, `Guarded`. The change
  is in the check point (`efr-conversation`, `turn/review.rs` and `exit.rs`), in the
  reviewer's prompt and in its output form, which has no `ask_user` any more.
- The tests that answer a question about an exit run with the reviewer off.
- The wire protocol does not change. `Verdict::AskUser` and `ExitInfo::judged` stay
  for the events of older conversations.

## Alternatives

1. **Remove the kernel sandbox and judge each command that the rules do not know.**
   It would remove about 56,000 lines and the slowest CI job. Rejected for now: its
   protection fails open, because a build, a script or `python -c` runs code that no
   judge reads. The reason of ADR 0006 still holds, and the model now also reads the
   web.
2. **A thin sandbox with no proxy, no bus filter and no port forwards.** It would
   remove about 21,000 lines, and the CI job would go from 17 to about 11 minutes.
   Rejected: a contained dev server would not be reachable, each fetch would run
   outside the sandbox, and shards for the CI job save more time with no loss.
3. **Keep the questions for the class "only the user".** Rejected: these exits were a
   large part of the questions, and the user said yes to nearly all of them.
4. **A key that lets the reviewer ask when it is not sure.** Not now: it brings the
   old behaviour back in a third form. `classifier.enabled = false` exists for a user
   who wants to answer.
