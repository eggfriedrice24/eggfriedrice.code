import { describe, expect, it } from "vitest";

import { ConnectionError, ServerError } from "@/lib/net/errors";
import type { ApprovalView } from "@/lib/stream/reducer";
import {
  answerErrorText,
  answerState,
  approvalBlock,
  approvalDiffRows,
  changed,
  NO_ANSWER,
  TERMINAL_NOTES,
  terminalReason,
  type AnswerMutation,
  type ApprovalBlockContext,
  type ApprovalBlockModel,
  type ClosedApproval,
  type OpenApproval,
} from "./approval-block";
import { MINUS, TIME_UP } from "./approval-block-text";

/** 14:32 and 13:02 to 13:12 on the phone's local clock, as on the board. */
const AT_1432 = new Date(2026, 9, 26, 14, 32).getTime();
const AT_1302 = new Date(2026, 9, 26, 13, 2).getTime();
const AT_1312 = new Date(2026, 9, 26, 13, 12).getTime();

const PUSH: ApprovalView = {
  callId: "c1",
  turnId: "t1",
  summary: 'shell: run "git push -u origin fix/steer-race"',
  diffPreview: undefined,
  reason: undefined,
  modelReason: undefined,
  exitKinds: [],
  always: undefined,
  localOnly: false,
  interactive: false,
  askedAt: AT_1302,
  expiresAt: undefined,
  status: "pending",
  decision: undefined,
  answeredBy: undefined,
  closedAt: undefined,
};

const CONTEXT: ApprovalBlockContext = {
  machine: "desktop",
  cwd: "/home/me/code/efr",
  connected: true,
  expiry: undefined,
  answer: NO_ANSWER,
};

function open(model: ApprovalBlockModel): OpenApproval {
  if (model.kind !== "open") {
    throw new Error(`the block is ${model.kind}`);
  }
  return model;
}

function closed(model: ApprovalBlockModel): ClosedApproval {
  if (model.kind !== "closed") {
    throw new Error(`the block is ${model.kind}`);
  }
  return model;
}

describe("an open approval block", () => {
  it("waits with a reason: header, command, cwd, reason and both buttons", () => {
    const model = open(
      approvalBlock(
        { ...PUSH, reason: "Pushes to a remote. cautious asks before any network access." },
        { ...CONTEXT, expiry: "expires in 4m" },
      ),
    );
    expect(model.label).toBe("Approval needed");
    expect(model.expiry).toBe("expires in 4m");
    expect(model.subject).toEqual({ kind: "command", line: "git push -u origin fix/steer-race" });
    expect(model.cwd).toBe("~/code/efr");
    expect(model.reason?.map((span) => span.text)).toEqual([
      "Pushes to a remote. ",
      "cautious",
      " asks before any network access.",
    ]);
    expect(model.terminal).toBeUndefined();
    expect(model.buttons).toEqual({ enabled: true, sending: undefined });
    expect([model.hint, model.error, model.diff]).toEqual([undefined, undefined, undefined]);
  });

  it("waits without a reason, and without a cwd when the turn has none", () => {
    const model = open(approvalBlock(PUSH, { ...CONTEXT, cwd: undefined }));
    expect(model.reason).toBeUndefined();
    expect(model.cwd).toBeUndefined();
    expect(model.buttons?.enabled).toBe(true);
  });

  it("keeps a long command whole", () => {
    const line =
      'docker system prune --all --volumes --filter "until=720h" --force && docker builder prune --all --force';
    const model = open(
      approvalBlock({ ...PUSH, summary: `shell: run ${JSON.stringify(line)}` }, CONTEXT),
    );
    expect(model.subject).toEqual({ kind: "command", line });
  });

  it("shows a file write as an edit line and a diff box", () => {
    const model = open(
      approvalBlock(
        {
          ...PUSH,
          summary: "write_file: write /home/me/.zshrc (user data)",
          diffPreview: [
            "--- a/.zshrc",
            "+++ b/.zshrc",
            "@@ -12,2 +12,3 @@",
            ' export PATH="$HOME/.cargo/bin:$PATH"',
            "-source ~/.zsh/nvm.zsh",
            "+zsh-defer source ~/.zsh/nvm.zsh",
            "+zsh-defer source ~/.zsh/pyenv.zsh",
            "",
          ].join("\n"),
          reason: "Writes outside this conversation's folder, ~/.dotfiles.",
        },
        CONTEXT,
      ),
    );
    expect(model.subject).toEqual({ kind: "edit", path: "~/.zshrc", added: 2, removed: 1 });
    expect(model.diff?.map((row) => [row.number, row.kind, row.sign])).toEqual([
      ["12", "context", " "],
      ["13", "removed", "-"],
      ["13", "added", "+"],
      ["14", "added", "+"],
    ]);
    expect(model.diff?.[2]?.runs).toEqual([
      { at: 0, text: "zsh-defer ", strong: true },
      { at: 10, text: "source ~/.zsh/nvm.zsh", strong: false },
    ]);
    expect(model.diff?.[1]?.runs).toEqual([
      { at: 0, text: "source ~/.zsh/nvm.zsh", strong: false },
    ]);
    expect(model.diff?.[3]?.runs).toEqual([
      { at: 0, text: "zsh-defer source ~/.zsh/pyenv.zsh", strong: false },
    ]);
    expect(model.reason?.filter((span) => span.mono).map((span) => span.text)).toEqual([
      "~/.dotfiles",
    ]);
  });

  it("shows the answer on its way: Allow pending, Deny dimmed", () => {
    const model = open(
      approvalBlock(PUSH, {
        ...CONTEXT,
        answer: { sending: "allow", answeredHere: false, failed: undefined },
      }),
    );
    expect(model.buttons).toEqual({ enabled: false, sending: "allow" });
    expect(model.hint).toBeUndefined();
  });

  it("closes the buttons while the machine is offline, and says when the user can answer", () => {
    const model = open(approvalBlock(PUSH, { ...CONTEXT, connected: false }));
    expect(model.buttons).toEqual({ enabled: false, sending: undefined });
    expect(model.hint).toBe("You can answer when desktop is connected again.");
  });

  it("closes the buttons when the time is up, before efrd closes the question", () => {
    const model = open(approvalBlock(PUSH, { ...CONTEXT, expiry: TIME_UP }));
    expect(model.buttons?.enabled).toBe(false);
    expect(model.expiry).toBe(TIME_UP);
  });

  it("shows why the last answer failed, and opens the buttons again", () => {
    const model = open(
      approvalBlock(PUSH, {
        ...CONTEXT,
        answer: {
          sending: undefined,
          answeredHere: false,
          failed: {
            error: new ConnectionError("offline", "no connection"),
            decision: "allow",
            commandId: "k1",
          },
        },
      }),
    );
    expect(model.error).toBe("The answer did not reach desktop. Try again.");
    expect(model.buttons?.enabled).toBe(true);
  });

  it("keeps the other subjects of the question as details", () => {
    const model = open(
      approvalBlock(
        {
          ...PUSH,
          summary: 'shell: run "curl https://x.dev"; network access\nasks for: curl',
        },
        CONTEXT,
      ),
    );
    expect(model.details).toEqual(["network access", "asks for: curl"]);
  });
});

describe("an approval that only the terminal answers", () => {
  it("sends a privilege question to the terminal, with no buttons", () => {
    const privilege: ApprovalView = {
      ...PUSH,
      summary: 'shell: run "sudo systemctl restart tailscaled"',
      localOnly: true,
      interactive: true,
    };
    expect(terminalReason(privilege)).toBe("privilege");
    const model = open(approvalBlock(privilege, { ...CONTEXT, cwd: "/home/me" }));
    expect(model.label).toBe("Approval needed, answer on desktop");
    expect(model.terminal).toEqual({
      line: "answer on desktop",
      note: "It needs your password, which only the terminal can take.",
    });
    expect(model.buttons).toBeUndefined();
    expect(model.cwd).toBe("~");
    expect(terminalReason({ ...PUSH, localOnly: true, exitKinds: ["privilege"] })).toBe(
      "privilege",
    );
  });

  it("sends a background job to the terminal", () => {
    const job: ApprovalView = {
      ...PUSH,
      summary: 'shell: run "restic backup ~/code --tag nightly &"',
      localOnly: true,
    };
    expect(terminalReason(job)).toBe("job");
    const model = open(approvalBlock(job, CONTEXT));
    expect(model.terminal?.note).toBe(TERMINAL_NOTES.job);
    expect(model.buttons).toBeUndefined();
  });

  it("sends a guarded delete to the terminal", () => {
    const guarded: ApprovalView = { ...PUSH, localOnly: true, expiresAt: AT_1312 };
    expect(terminalReason(guarded)).toBe("delete");
    expect(open(approvalBlock(guarded, CONTEXT)).buttons).toBeUndefined();
  });

  it("keeps the buttons for an always rule, which efrd takes from a phone as a one-time answer", () => {
    const always: ApprovalView = {
      ...PUSH,
      summary: 'shell: run "cargo publish -p efr-proto"',
      always: "run cargo publish",
    };
    expect(terminalReason(always)).toBe("always");
    const model = open(approvalBlock(always, CONTEXT));
    expect(model.label).toBe("Approval needed");
    expect(model.terminal).toEqual({
      line: "always rule on desktop",
      note: 'Answering can add an "always" rule, and rules are only set in the terminal.',
    });
    expect(model.buttons).toEqual({ enabled: true, sending: undefined });
    expect(terminalReason(PUSH)).toBeUndefined();
  });

  it("shows no error and no hint for a local question", () => {
    const model = open(
      approvalBlock(
        { ...PUSH, localOnly: true },
        {
          ...CONTEXT,
          connected: false,
          answer: {
            sending: undefined,
            answeredHere: false,
            failed: { error: new Error("x"), decision: "deny", commandId: "k" },
          },
        },
      ),
    );
    expect([model.hint, model.error]).toEqual([undefined, undefined]);
  });
});

describe("a closed approval block", () => {
  it("is a quiet line once answered, here or in a terminal", () => {
    const here = closed(
      approvalBlock(
        { ...PUSH, status: "allowed", decision: "allow", answeredBy: "phone", closedAt: AT_1432 },
        { ...CONTEXT, answer: { sending: undefined, answeredHere: true, failed: undefined } },
      ),
    );
    expect(here).toEqual({
      kind: "closed",
      glyph: "allowed",
      text: "allowed on this phone · 14:32",
      subject: "$ git push -u origin fix/steer-race",
      label: "Allowed on this phone, 14:32: $ git push -u origin fix/steer-race",
    });
    const terminal = closed(
      approvalBlock(
        { ...PUSH, status: "denied", decision: "deny", answeredBy: "shell", closedAt: AT_1432 },
        CONTEXT,
      ),
    );
    expect([terminal.glyph, terminal.text]).toEqual([
      "denied",
      "denied in a terminal on desktop · 14:32",
    ]);
  });

  it("tells another phone, an always answer and an unknown surface apart", () => {
    const other = closed(
      approvalBlock(
        { ...PUSH, status: "allowed", decision: "allow", answeredBy: "phone" },
        CONTEXT,
      ),
    );
    expect(other.text).toBe("allowed on a phone");
    const always = closed(
      approvalBlock(
        { ...PUSH, status: "allowed", decision: "allow_always", answeredBy: "cli" },
        CONTEXT,
      ),
    );
    expect(always.text).toBe("always allowed in a terminal on desktop");
    const unknown = closed(approvalBlock({ ...PUSH, status: "denied", decision: "deny" }, CONTEXT));
    expect(unknown.text).toBe("denied");
  });

  it("tells an expiry and an expiry while away apart, with their times", () => {
    const expired = closed(
      approvalBlock({ ...PUSH, status: "expired", closedAt: AT_1312 }, CONTEXT),
    );
    expect([expired.glyph, expired.text]).toEqual(["expired", "expired · not answered · 13:12"]);
    const away = closed(
      approvalBlock({ ...PUSH, status: "expired_away", closedAt: AT_1312 }, CONTEXT),
    );
    expect(away.text).toBe("expired while you were away · 13:02-13:12");
    const awayNoStart = closed(
      approvalBlock(
        { ...PUSH, status: "expired_away", askedAt: undefined, closedAt: AT_1312 },
        CONTEXT,
      ),
    );
    expect(awayNoStart.text).toBe("expired while you were away · 13:12");
  });

  it("says when the phone did not see the answer", () => {
    const settled = closed(approvalBlock({ ...PUSH, status: "settled" }, CONTEXT));
    expect([settled.glyph, settled.text]).toEqual([
      "expired",
      "closed · the phone did not see the answer",
    ]);
  });

  it("shows the edit line of a file write on its quiet line", () => {
    const model = closed(
      approvalBlock(
        {
          ...PUSH,
          summary: "write_file: write /home/me/.zshrc",
          diffPreview: "@@ -1 +1 @@\n-a\n+b\n",
          status: "denied",
          decision: "deny",
        },
        CONTEXT,
      ),
    );
    expect(model.subject).toBe(`edit ~/.zshrc +1 ${MINUS}1`);
  });
});

describe("approvalDiffRows", () => {
  it("drops file headers and shows hunk headers only for more than one hunk", () => {
    const rows = approvalDiffRows(
      "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n@@ -9 +9 @@\n-c\n\\ No newline at end of file\n+d\n",
    );
    expect(rows.map((row) => [row.key, row.kind, row.number])).toEqual([
      ["0", "hunk", ""],
      ["1", "removed", "1"],
      ["2", "added", "1"],
      ["3", "hunk", ""],
      ["4", "removed", "9"],
      ["5", "note", ""],
      ["6", "added", "9"],
    ]);
    expect(approvalDiffRows("@@ -1 +1 @@\n-a\n+b\n").map((row) => row.kind)).toEqual([
      "removed",
      "added",
    ]);
  });

  it("keeps lines without counts and text that is not a diff", () => {
    expect(approvalDiffRows("+new\nplain text\n").map((row) => [row.kind, row.number])).toEqual([
      ["added", ""],
      ["other", ""],
    ]);
  });
});

describe("changed", () => {
  it("marks the middle that changed, and nothing for lines that share nothing", () => {
    expect(changed("let a = 1;", "let b = 1;")).toEqual([
      [
        { at: 0, text: "let ", strong: false },
        { at: 4, text: "a", strong: true },
        { at: 5, text: " = 1;", strong: false },
      ],
      [
        { at: 0, text: "let ", strong: false },
        { at: 4, text: "b", strong: true },
        { at: 5, text: " = 1;", strong: false },
      ],
    ]);
    expect(changed("abc", "xyz")).toBeUndefined();
  });

  it("does not cut a surrogate pair", () => {
    const pair = changed("x\u{1f95a}", "x\u{1f95b}");
    expect(pair?.[0].map((run) => run.text)).toEqual(["x", "\u{1f95a}"]);
    const tail = changed("\u{1f95a}y", "\u{1f95b}y");
    expect(tail?.[1].map((run) => run.text)).toEqual(["\u{1f95b}", "y"]);
  });
});

/** One answer of the mutation cache. */
function answer(
  status: AnswerMutation["status"],
  submittedAt: number,
  decision: "allow" | "deny" = "allow",
  callId = "c1",
  error: unknown = null,
): AnswerMutation {
  return {
    variables: { callId, decision, commandId: `k${submittedAt}` },
    status,
    error,
    submittedAt,
  };
}

describe("answerState", () => {
  it("knows no answer", () => {
    expect(answerState([], "c1", true)).toBe(NO_ANSWER);
    expect(answerState([answer("pending", 1, "allow", "c2")], "c1", true)).toBe(NO_ANSWER);
    expect(answerState([{ ...answer("pending", 1), variables: { x: 1 } }], "c1", true)).toBe(
      NO_ANSWER,
    );
  });

  it("is sending while the answer runs, and until the stream shows the question closed", () => {
    expect(answerState([answer("pending", 1)], "c1", true).sending).toBe("allow");
    const taken = answerState([answer("success", 1, "deny")], "c1", true);
    expect([taken.sending, taken.answeredHere]).toEqual(["deny", true]);
    const resolved = answerState([answer("success", 1, "deny")], "c1", false);
    expect([resolved.sending, resolved.answeredHere]).toEqual([undefined, true]);
  });

  it("gives the newest failure with its command id, while the question is open", () => {
    const error = new Error("lost");
    const state = answerState(
      [
        answer("error", 1, "deny", "c1", new Error("old")),
        answer("error", 2, "allow", "c1", error),
      ],
      "c1",
      true,
    );
    expect(state.failed).toEqual({ error, decision: "allow", commandId: "k2" });
    expect(state.sending).toBeUndefined();
    expect(answerState([answer("error", 1)], "c1", false).failed).toBeUndefined();
    expect(answerState([answer("error", 1), answer("pending", 2)], "c1", true)).toEqual({
      sending: "allow",
      answeredHere: false,
      failed: undefined,
    });
  });
});

function server(code: "forbidden" | "not_found" | "conflict" | "busy"): ServerError {
  return new ServerError({ code, message: code });
}

describe("answerErrorText", () => {
  it("says what went wrong in one sentence", () => {
    expect(answerErrorText(server("forbidden"), "desktop")).toBe(
      "efrd takes no answer to this question from a phone. Answer on desktop.",
    );
    expect(answerErrorText(server("not_found"), "desktop")).toBe("The question is not open now.");
    expect(answerErrorText(server("conflict"), "desktop")).toBe("The question is not open now.");
    expect(answerErrorText(server("busy"), "desktop")).toBe(
      "efrd did not take the answer. Try again.",
    );
    expect(answerErrorText(new ConnectionError("socket", "closed"), "laptop")).toBe(
      "The answer did not reach laptop. Try again.",
    );
    expect(answerErrorText(new Error("x"), "desktop")).toBe("The answer failed. Try again.");
  });
});
