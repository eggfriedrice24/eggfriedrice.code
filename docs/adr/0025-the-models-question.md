# 0025: The model's question: an answer is no permission

Status: accepted, 2026-11-17.

## Context

In 0.0.11 the model could not ask the user a question in the middle of a turn. It had
to guess, or to end the turn with a question in its answer. A guess that the user did
not want costs a turn, and in a job notice turn nobody reads the answer for a while.

efr already has questions: the approvals of the permission engine, the quarantine
question and the input of a command. Each one of them is a question of efr, about an
action. A question of the model is different: the model writes the text and the
options. A model that read a hostile file, such as a README or the output of a
command, can write a question that leads the user to type words of the attacker. So
the design must say what an answer can change.

ADR 0023 keeps the reviewer away from model text, and one check that no model makes,
the naming rule, holds the guarded exits (a package publish, the session bus, a
credential agent socket). That rule reads only the user's own messages.

## Decision

1. **A tool of efr, `ask_user`.** The model calls it with one question (at most 300
   characters) and 2 to 4 options (a label of at most 60 characters, an optional
   description of at most 200). efr checks the input again and refuses a bad one with
   an error result; nobody is asked. efr-conversation owns the tool, because the toolbox
   cannot reach the questions or the presence of a conversation. The efr-tools
   registry refuses a tool with the name `ask_user` (`efr_tools::ASK_USER`), so no later
   tool or server can take it. `conversation.ask_user = false` removes the tool.

2. **An answer is information, not a permission.** The question goes before no
   permission check, and the permission engine and the toolbox never see the call. An
   answer allows no action, changes no mode and saves no rule. The question pauses the
   turn in every mode, `auto` and `full` too. In a full turn, the gate before each
   call runs first: a full turn whose gate went off stops and asks nothing.

3. **An answer is never a user message for efr.** A picked option and a typed answer
   do not enter the naming window of a guarded exit, and the reviewer's record of an
   exit does not hold them, in this turn or in a later one, from every client. An
   answer also does not start the counts of refusals and denials in a row again, as an
   answer to a question of efr does, so the model cannot use a question to lift the
   pause of the reviewer. The reason: the model wrote the question, so a typed answer
   can be words that a hostile file asked for. With the answer as a user message, "type
   the command that publishes" would lift a guarded refusal. The user asks for such an
   action with a message, as ADR 0023 says for each refusal.

4. **One wait rule.** The question waits as an approval
   (`conversation.approval_timeout_secs`, no end by default) while a client that can answer it follows the conversation. When
   no such client follows, or the user is away, it waits `presence.away_wait_secs`
   (300 s) from that moment, with one `presence.push` that names the call. Then it ends
   as unanswered, and the model goes on with a choice that is safe and easy to undo.
   An answer wins during the whole wait. After one unanswered question, each later
   `ask_user` of the same turn gets the unanswered result at once. The same loop
   (`turn/wait.rs`) serves the approvals, the quarantine question and this question.

5. **Who can answer.** A client says so with `answers_questions` in
   `conversation.subscribe`. efrd counts the flag only on a connection that holds
   `operate`: a terminal of the user, or a phone whose device scopes hold it. A
   model-side peer holds `read` only and never counts. `question.respond` needs
   `operate`, not `approve`: an answer has less power than a steer, which a phone
   already sends. A phone can pick an option, type an answer or skip.

6. **A fixed hint, no filter.** Both clients show `Do not type a password or a token
   here.` where the user types an answer, and the tool text tells the model never to
   ask for a secret. efr does not refuse a question for words such as "token": the
   word is common in efr's own subject, and a hostile question can avoid it.

## Consequences

- After an answer, efr can still refuse an exit that the answer asked for. The model
  tells the user, and one message of the user is the way to ask.
- A typed answer goes to the event log, the receipt and the provider, as a prompt does.
- A job notice turn can ask. With nobody there, it stops for 300 s, and the desktop
  notice of the terminal tells the user in that time.
- The quarantine question now uses the same wait. While the user is away, it ends after
  `presence.away_wait_secs`, and the change stays in quarantine. Before 0.0.12 it
  waited without end.
- The 0.0.11 `efr` and the 0.0.11 app send no `answers_questions`, so a question that
  only they follow takes the away wait.
- `ask_user` was also a verdict of the reviewer in old `exit_judged` events (ADR 0023).
  The two names are in different members of different events and do not collide.
- 0.0.12 does not put the question, its options or a typed answer into the search
  index. `efr search` and the search of the phone do not find them. A later version can
  index the question as text of the model and the answer as a kind of its own, never as
  a prompt or a steer, so a result never shows an answer as a message of the user
  (decision 3).

## Alternatives

1. **An answer counts as a user message, as a steer does.** Rejected: decision 3.
2. **A marked form of the answer for the reviewer.** Rejected: it needs a new line of
   the reviewer's prompt and a billed eval, and a message of the user does the same
   today.
3. **Only the terminal answers.** Rejected: the phone already sends prompts and steers,
   which have more power. The reason that keeps `input.respond` local, a password in
   the buffers of the WebSocket (ADR 0022), is covered by the hint and the tool text.
4. **Several questions per call, multi-select, previews.** Left out: more UI on two
   clients for little gain. The model can call the tool twice.
