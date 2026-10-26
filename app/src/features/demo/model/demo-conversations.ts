// The conversations of the demo machines: one script for each kind of content that a
// board of the design shows (design-spec section 1, "States drawn"). The sample data is
// one consistent set (plan 5.2 item x5). Each script has a history, which the demo
// server sends at once when the gallery sets the stage, and a live part, which it plays
// from the first subscription. A question in either part waits for the phone's answer.
// Pure.

import { DemoScript, type AskOptions, type DemoStep, type NewId } from "./demo-script";

/** One conversation of a stage. */
export interface DemoConversationScript {
  readonly title: string;
  /** Sent at once when the stage starts, as if it happened before. */
  readonly history: readonly DemoStep[];
  /** Played from the first subscription. */
  readonly live: readonly DemoStep[];
}

/** The kinds of conversation that the stages use. */
export type DemoConversationKind =
  | "streaming"
  | "approval"
  | "waiting"
  | "notes"
  | "disk"
  | "rename"
  | "failed"
  | "long"
  | "markdown"
  | "tools"
  | "messages"
  | "diff";

const SUM_REPLY = [
  "The test failed because the loop read past the end of the buffer. Here is the fix:",
  "",
  "```rust",
  "pub fn sum(values: &[u32]) -> u64 {",
  "    let mut total = 0u64;",
  "    for value in values {",
  "        total += u64::from(*value);",
  "    }",
  "    total",
  "}",
  "```",
  "",
  "I run the tests of `efr-store` again to check it.",
].join("\n");

const MARKDOWN_REPLY = [
  "## What uses the disk",
  "",
  "The journal is the **largest** part. The cache of pacman comes next.",
  "",
  "| Folder | Size | Share |",
  "|---|---:|---:|",
  "| /var/log/journal | 3.1 GiB | 62% |",
  "| /var/cache/pacman | 1.2 GiB | 24% |",
  "| /tmp | 0.7 GiB | 14% |",
  "",
  "### What you can do",
  "",
  "1. Set `SystemMaxUse=500M` in `journald.conf`.",
  "2. Clear the old packages with `paccache -r`.",
  "3. Restart the journal:",
  "",
  "```bash",
  "sudo systemctl restart systemd-journald",
  "```",
  "",
  "> The journal keeps the last 500 MB after the change.",
  "",
  "- See [the journald docs](https://www.freedesktop.org/software/systemd/man/journald.conf.html) for the keys.",
  "- *Italic* and ~~struck~~ text work too.",
].join("\n");

const DIFF = [
  "--- a/crates/efr-store/src/sum.rs",
  "+++ b/crates/efr-store/src/sum.rs",
  "@@ -1,7 +1,7 @@",
  " pub fn sum(values: &[u32]) -> u64 {",
  "     let mut total = 0u64;",
  "-    for i in 0..=values.len() {",
  "-        total += u64::from(values[i]);",
  "+    for value in values {",
  "+        total += u64::from(*value);",
  "     }",
  "     total",
  " }",
  "",
].join("\n");

const JOURNAL_DIFF = [
  "--- a/etc/systemd/journald.conf",
  "+++ b/etc/systemd/journald.conf",
  "@@ -14,3 +14,3 @@",
  " [Journal]",
  "-#SystemMaxUse=",
  "+SystemMaxUse=500M",
  " #SystemKeepFree=",
  "",
].join("\n");

const TEST_OUTPUT = [
  "   Compiling efr-store v0.0.5 (/home/me/code/efr/crates/efr-store)",
  "    Finished `test` profile [unoptimized + debuginfo] target(s) in 4.12s",
  "    Starting 38 tests across 1 binary",
  "        PASS [   0.004s] efr-store sum::adds_values",
  "        FAIL [   0.003s] efr-store sum::empty_slice",
  "error: test run failed",
].join("\n");

function longReply(): string {
  const parts: string[] = ["# The steps of the release", ""];
  for (let i = 1; i <= 40; i++) {
    parts.push(
      `${i}. Step ${i} reads the config, checks the **value** of \`key_${i}\`, and writes one line to the log. It does not change the file when the value is the same.`,
    );
    if (i % 10 === 0) {
      parts.push("", `## Part ${i / 10}`, "", "> Each part ends with a check of the log.", "");
    }
  }
  return parts.join("\n");
}

/** A turn that is over: a prompt, a reply and the end. */
function pastTurn(script: DemoScript, id: NewId, prompt: string, reply: string): DemoScript {
  const turn = id();
  return script.prompt(turn, prompt, id()).said(turn, 0, reply).done(turn, 42_000);
}

/** The script of a conversation of kind `kind`. */
export function demoConversation(kind: DemoConversationKind, id: NewId): DemoConversationScript {
  switch (kind) {
    case "streaming":
      return streaming(id);
    case "approval":
      return approval(id, false);
    case "waiting":
      return approval(id, true);
    case "notes":
      return finished(id, "update my notes and restart the web server", [
        "I updated `notes.md` with the new port and restarted the web server. It answers on port 8080 now.",
      ]);
    case "disk":
      return finished(id, "why is the disk full", [
        "The journal takes 3.1 GiB of the 5 GiB that are in use. Set `SystemMaxUse=500M` to keep it small.",
      ]);
    case "rename":
      return finished(id, "rename the config keys to snake case", [
        "I renamed 14 keys in `config.toml` and in the docs. The tests pass.",
      ]);
    case "failed":
      return failed(id);
    case "long":
      return long(id);
    case "markdown":
      return finished(id, "what uses the disk", [MARKDOWN_REPLY]);
    case "tools":
      return tools(id);
    case "messages":
      return messages(id);
    case "diff":
      return diff(id);
    default:
      return finished(id, kind, ["Done."]);
  }
}

function finished(id: NewId, prompt: string, replies: readonly string[]): DemoConversationScript {
  const script = new DemoScript().created();
  for (const reply of replies) {
    pastTurn(script, id, prompt, reply);
  }
  return { title: prompt, history: script.steps(), live: [] };
}

/** R-Streaming: a thought, tools, a live code block, the running line and a queued prompt. */
function streaming(id: NewId): DemoConversationScript {
  const history = pastTurn(
    new DemoScript().created(),
    id,
    "where is the sum function of efr-store",
    "It is in `crates/efr-store/src/sum.rs`.",
  );
  const turn = id();
  const next = id();
  const live = new DemoScript()
    .wait(600)
    .prompt(turn, "fix the flaky sum test", id())
    .reason(
      turn,
      "**Reading the test**\n\nThe test reads one value past the end. The loop bound is wrong.",
      "Reading the test",
      { every: 60 },
    )
    .tool(
      turn,
      id(),
      "read_file",
      { path: "crates/efr-store/src/sum.rs" },
      "pub fn sum(values: &[u32]) -> u64 {\n    let mut total = 0u64;\n    for i in 0..=values.len() {\n        total += u64::from(values[i]);\n    }\n    total\n}",
    )
    .tool(turn, id(), "shell", { command: "cargo nextest run -p efr-store sum" }, TEST_OUTPUT, {
      isError: true,
      lineMs: 500,
    })
    .wait(400)
    .queued(next, "then run the whole suite", id(), "phone")
    .say(turn, 0, SUM_REPLY, { piece: 4, every: 70 })
    .done(turn, 91_200)
    .start(next)
    .say(next, 0, "All 1,204 tests pass.", { every: 60 })
    .done(next, 93_000);
  return { title: "fix the flaky sum test", history: history.steps(), live: live.steps() };
}

/** R-Approval: a question in the middle of a turn, with a reason, that waits for you. */
function approval(id: NewId, askAtOnce: boolean): DemoConversationScript {
  const turn = id();
  const call = id();
  const ask = (script: DemoScript): DemoScript =>
    script
      .prompt(turn, "keep the journal small", id())
      .say(turn, 0, "I check how much space the journal takes first.")
      .tool(
        turn,
        id(),
        "shell",
        { command: "journalctl --disk-usage" },
        "Archived and active journals take up 3.1G in the file system.",
      )
      .say(turn, 1, "It takes 3.1 GB. I set a limit of 500 MB in the config.")
      .wait(300)
      .ask(
        turn,
        call,
        "write_file",
        { path: "/etc/systemd/journald.conf" },
        {
          summary: "write `/etc/systemd/journald.conf`",
          reason:
            "Writes outside the project. cautious asks before a write outside the working directory.",
          diffPreview: JOURNAL_DIFF,
        },
      )
      .hold(
        turn,
        call,
        new DemoScript()
          .output(turn, call, "Wrote 1 line to /etc/systemd/journald.conf.")
          .say(turn, 2, "Done. The journal keeps at most 500 MB now.")
          .done(turn, 64_000),
        new DemoScript()
          .event({
            kind: "tool_call_completed",
            turn_id: turn,
            call_id: call,
            output: "The user denied the call.",
            truncated: false,
            is_error: true,
          })
          .say(turn, 2, "I left the config as it was.")
          .done(turn, 61_000),
      );
  const base = new DemoScript().created();
  if (askAtOnce) {
    return { title: "keep the journal small", history: ask(base).steps(), live: [] };
  }
  return {
    title: "keep the journal small",
    history: base.steps(),
    live: ask(new DemoScript().wait(500)).steps(),
  };
}

/** C-Failed: an interrupted turn and a failed one. */
function failed(id: NewId): DemoConversationScript {
  const first = id();
  const second = id();
  const script = new DemoScript()
    .created()
    .prompt(first, "upgrade every crate to the latest version", id())
    .say(first, 0, "I start with the crates of the workspace.")
    .toolDone(
      first,
      id(),
      "shell",
      { command: "cargo update --workspace" },
      "    Updating crates.io index",
    )
    .interrupt(first, "phone")
    .prompt(second, "only upgrade serde", id())
    .say(second, 0, "I upgrade serde to 1.0.229.")
    .fail(second, "The model provider answered 529: it is overloaded. Try again in a moment.");
  return { title: "upgrade every crate", history: script.steps(), live: [] };
}

/** C-Jump: a long transcript, then a reply that streams while you read above it. */
function long(id: NewId): DemoConversationScript {
  const history = pastTurn(
    new DemoScript().created(),
    id,
    "explain the release steps",
    longReply(),
  );
  const turn = id();
  const live = new DemoScript()
    .wait(800)
    .prompt(turn, "and the checks after each part", id())
    .say(turn, 0, longReply(), { piece: 6, every: 50 })
    .done(turn, 120_000);
  return { title: "explain the release steps", history: history.steps(), live: live.steps() };
}

/** S-Tools: a tool block of every status. */
function tools(id: NewId): DemoConversationScript {
  const turn = id();
  const script = new DemoScript()
    .created()
    .prompt(turn, "check the build", id())
    .toolDone(
      turn,
      id(),
      "read_file",
      { path: "crates/efr-store/src/sum.rs" },
      "pub fn sum(values: &[u32]) -> u64 {\n    values.iter().map(|v| u64::from(*v)).sum()\n}",
    )
    .toolDone(
      turn,
      id(),
      "shell",
      { command: "cargo build -p efr-store" },
      "   Compiling efr-store v0.0.5\n    Finished `dev` profile in 3.20s",
    )
    .toolDone(turn, id(), "shell", { command: "cargo nextest run -p efr-store" }, TEST_OUTPUT, {
      is_error: true,
      exit_code: 101,
    })
    .toolDone(
      turn,
      id(),
      "apply_patch",
      { path: "crates/efr-store/src/sum.rs" },
      "Applied 1 hunk.",
      { diff: DIFF },
    )
    .event({
      kind: "tool_call_started",
      turn_id: turn,
      call_id: id(),
      tool: "shell",
      input: { command: "cargo nextest run --workspace" },
    })
    .draft(turn, { kind: "tool_input", call: 1, tool: "apply_patch", bytes: 2_048 });
  return { title: "check the build", history: script.steps(), live: [] };
}

/** S-Messages: thoughts, your messages (queued, steered) and every turn end. */
function messages(id: NewId): DemoConversationScript {
  const first = id();
  const second = id();
  const third = id();
  const fourth = id();
  const script = new DemoScript()
    .created()
    .prompt(first, "why is the disk full", id())
    .reason(first, "**Checking the logs**\n\nThe journal holds most of it.", "Checking the logs")
    .said(first, 0, "The journal takes 3.1 GiB.")
    .done(first, 21_000)
    .prompt(second, "upgrade every crate", id())
    .event({ kind: "turn_steered", turn_id: second, text: "only the crates of the workspace" })
    .event({ kind: "steering_delivered", turn_id: second, steers: [] })
    .said(second, 0, "I upgrade the crates of the workspace only.")
    .interrupt(second, "phone")
    .prompt(third, "try again with serde only", id())
    .fail(third, "The model provider answered 529: it is overloaded.")
    .prompt(fourth, "run the tests", id())
    .reason(fourth, "Running the tests of the workspace.")
    .queued(id(), "then format the code", id(), "phone");
  return { title: "why is the disk full", history: script.steps(), live: [] };
}

/** S-Diff: an edit block with its diff. */
function diff(id: NewId): DemoConversationScript {
  const turn = id();
  const script = new DemoScript()
    .created()
    .prompt(turn, "fix the sum function", id())
    .toolDone(
      turn,
      id(),
      "apply_patch",
      { path: "crates/efr-store/src/sum.rs" },
      "Applied 1 hunk.",
      { diff: DIFF },
    )
    .said(turn, 0, "The loop now reads each value once.")
    .done(turn, 30_000);
  return { title: "fix the sum function", history: script.steps(), live: [] };
}

/** One state of the approval block, for the S-Approvals board. */
export interface ApprovalCase {
  /** The name of the state on the board. */
  readonly name: string;
  readonly turnId: string;
  readonly callId: string;
  readonly steps: readonly DemoStep[];
  /** The answer that this phone sends now, for the state "sending the answer". */
  readonly sending?: "allow" | "deny";
  /** This phone answered, for the state "answered here". */
  readonly answeredHere?: boolean;
}

/** The fields of one question of a case. */
interface CaseQuestion {
  readonly tool: string;
  readonly input: unknown;
  readonly ask: AskOptions;
}

const RESTART: CaseQuestion = {
  tool: "shell",
  input: { command: "systemctl --user restart web" },
  ask: {
    summary: "run `systemctl --user restart web`",
    reason: "Changes a service. cautious asks before a command that changes the system.",
  },
};

const RESTART_BARE: CaseQuestion = {
  tool: "shell",
  input: { command: "systemctl --user restart web" },
  ask: { summary: "run `systemctl --user restart web`" },
};

/** S-Approvals: every state of the approval block (design-spec section 1). */
export function approvalCases(id: NewId): ApprovalCase[] {
  const build = (
    name: string,
    question: CaseQuestion,
    after?: (script: DemoScript, turn: string, call: string) => void,
    extra: { readonly sending?: "allow" | "deny"; readonly answeredHere?: boolean } = {},
  ): ApprovalCase => {
    const turn = id();
    const call = id();
    const script = new DemoScript()
      .created()
      .prompt(turn, "restart the web server", id())
      .ask(turn, call, question.tool, question.input, question.ask);
    after?.(script, turn, call);
    const result: { -readonly [K in keyof ApprovalCase]: ApprovalCase[K] } = {
      name,
      turnId: turn,
      callId: call,
      steps: script.steps(),
    };
    if (extra.sending !== undefined) {
      result.sending = extra.sending;
    }
    if (extra.answeredHere !== undefined) {
      result.answeredHere = extra.answeredHere;
    }
    return result;
  };
  return [
    build("waiting, with a reason", RESTART),
    build("waiting, without a reason", RESTART_BARE),
    build("long command", {
      tool: "shell",
      input: {
        command:
          "find target -name '*.rlib' -mtime +30 -print0 | xargs -0 rm -f -- && du -sh target && cargo clean -p efr-store --release",
      },
      ask: {
        summary:
          "run `find target -name '*.rlib' -mtime +30 -print0 | xargs -0 rm -f -- && du -sh target && cargo clean -p efr-store --release`",
        reason: "Deletes files. cautious asks before a command that deletes files.",
      },
    }),
    build("file write with a diff", {
      tool: "write_file",
      input: { path: "/etc/systemd/journald.conf" },
      ask: {
        summary: "write `/etc/systemd/journald.conf`",
        reason:
          "Writes outside the project. cautious asks before a write outside the working directory.",
        diffPreview: JOURNAL_DIFF,
      },
    }),
    build("sending the answer", RESTART, undefined, { sending: "allow" }),
    build(
      "answered: allowed here",
      RESTART,
      (script, turn, call) => script.answered(turn, call, "allow", "phone"),
      { answeredHere: true },
    ),
    build("answered: denied in a terminal", RESTART, (script, turn, call) =>
      script.answered(turn, call, "deny", "shell"),
    ),
    build("answer in the terminal: privilege", {
      tool: "shell",
      input: { command: "sudo journalctl --vacuum-size=500M" },
      ask: {
        summary: "run `sudo journalctl --vacuum-size=500M`",
        interactive: true,
        localOnly: true,
        exit: { kinds: ["privilege"], launch: { kind: "unsandboxed" } },
      },
    }),
    build("answer in the terminal: background job", {
      tool: "shell",
      input: { command: "pnpm dev" },
      ask: {
        summary: "run `pnpm dev` in the background",
        localOnly: true,
        exit: { kinds: ["persistence"], launch: { kind: "unsandboxed" } },
      },
    }),
    build("answer in the terminal: always rule", {
      tool: "shell",
      input: { command: "git push origin ui/demo" },
      ask: {
        summary: "run `git push origin ui/demo`",
        reason: "Pushes to a remote. cautious asks before any network access.",
        exit: { kinds: ["host"], launch: { kind: "contained" }, always: "git push to github.com" },
      },
    }),
    build(
      "expired",
      { ...RESTART_BARE, ask: { ...RESTART_BARE.ask, expiresInSecs: 120 } },
      (script, turn, call) =>
        script.event({ kind: "approval_expired", turn_id: turn, call_id: call }),
    ),
    build("expired while you were away", RESTART_BARE, (script, turn, call) =>
      script.event({ kind: "approval_expired", turn_id: turn, call_id: call, away: true }),
    ),
  ];
}
