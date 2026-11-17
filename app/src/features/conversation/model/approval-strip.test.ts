import { describe, expect, it } from "vitest";

import type { ApprovalView } from "@/lib/stream/reducer";
import { NO_ANSWER, type ApprovalBlockContext } from "./approval-block";
import { TIME_UP } from "./approval-block-text";
import {
  approvalCall,
  approvalSheet,
  approvalStrip,
  approvalQuestion,
  nextQuestion,
  shortTimeLeft,
} from "./approval-strip";

const PUSH: ApprovalView = {
  callId: "c1",
  turnId: "t1",
  summary: 'shell: run "git push -u origin fix/steer-race"',
  diffPreview: undefined,
  reason: "cautious asks before a push to a remote.",
  modelReason: undefined,
  exitKinds: [],
  always: undefined,
  grants: [],
  facts: [],
  judged: undefined,
  launch: undefined,
  userOnly: false,
  localOnly: false,
  interactive: false,
  askedAt: 1_000,
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

const ONE = { index: 0, count: 1 } as const;

describe("shortTimeLeft", () => {
  it("gives the seconds only under a minute, then time is up", () => {
    expect(shortTimeLeft(undefined)).toBeUndefined();
    expect(shortTimeLeft("expires in 4m")).toBeUndefined();
    expect(shortTimeLeft("expires in 42s")).toBe("42s");
    expect(shortTimeLeft(TIME_UP)).toBe(TIME_UP);
  });
});

describe("approvalStrip", () => {
  it("shows the command without its $ and no small text by default", () => {
    const strip = approvalStrip(PUSH, CONTEXT, ONE);
    expect(strip).toMatchObject({
      line: "git push -u origin fix/steer-race",
      label: "Approval needed: git push -u origin fix/steer-race",
      meta: [],
      terminal: undefined,
      buttons: { enabled: true, sending: undefined },
    });
  });

  it("adds the seconds, the place and offline only when they apply", () => {
    const strip = approvalStrip(
      PUSH,
      { ...CONTEXT, connected: false, expiry: "expires in 42s" },
      { index: 0, count: 3 },
    );
    expect(strip?.meta.map((part) => [part.text, part.tone])).toEqual([
      ["42s", "accent_primary"],
      ["1 of 3", "text_subtle"],
      ["offline", "text_subtle"],
    ]);
    expect(strip?.buttons?.enabled).toBe(false);
  });

  it("names the machine in place of the buttons for a question of the terminal", () => {
    const strip = approvalStrip(
      { ...PUSH, localOnly: true, interactive: true, summary: 'shell: run "sudo reboot"' },
      { ...CONTEXT, machine: "laptop" },
      ONE,
    );
    expect(strip).toMatchObject({ line: "sudo reboot", terminal: "answer on laptop" });
    expect(strip?.buttons).toBeUndefined();
  });

  it("keeps the buttons when an always rule is offered, which the terminal sets", () => {
    const strip = approvalStrip({ ...PUSH, always: "host github.com:443" }, CONTEXT, ONE);
    expect(strip?.terminal).toBeUndefined();
    expect(strip?.buttons).toBeDefined();
  });

  it("has no strip for a question that does not wait", () => {
    expect(approvalStrip({ ...PUSH, status: "allowed" }, CONTEXT, ONE)).toBeUndefined();
  });
});

describe("approvalSheet", () => {
  it("has the full command and one quiet line: where it runs and why it asks", () => {
    const sheet = approvalSheet(PUSH, CONTEXT, [PUSH]);
    expect(sheet.subject).toEqual({ kind: "command", line: "git push -u origin fix/steer-race" });
    expect(sheet.quiet).toBe("in ~/code/efr · cautious asks before a push to a remote");
    expect(sheet.open?.meta).toEqual([]);
    expect(sheet.closed).toBeUndefined();
  });

  it("says how a question ended while it showed, how many more wait, and which is next", () => {
    const other = { ...PUSH, callId: "c2" };
    const third = { ...PUSH, callId: "c3" };
    const expired = { ...PUSH, status: "expired" as const, closedAt: 9_000 };
    const sheet = approvalSheet(expired, CONTEXT, [other, third]);
    expect(sheet.open).toBeUndefined();
    expect(sheet.closed).toEqual({ parts: ["expired", "2 more waiting"] });
    expect(sheet.subject.kind).toBe("command");
    // Next of the native header: only after the question closed, and only when more wait.
    const q = approvalQuestion;
    expect(nextQuestion(q(expired), [q(other), q(third)])).toBe("c2");
    expect(nextQuestion(q(expired), [q(expired)])).toBeUndefined();
    expect(nextQuestion(q(PUSH), [q(PUSH), q(other)])).toBeUndefined();
    expect(nextQuestion(undefined, [q(other)])).toBeUndefined();
  });

  it("offers Next after a quarantine question closed, to an approval or another question", () => {
    const left = { callId: "q1", open: false };
    const waitingApproval = approvalQuestion({ ...PUSH, callId: "c2" });
    const waitingQuarantine = { callId: "q2", open: true };
    expect(nextQuestion(left, [waitingApproval, waitingQuarantine])).toBe("c2");
    expect(nextQuestion(left, [waitingQuarantine])).toBe("q2");
    // While the quarantine question waits, Next does not show.
    expect(nextQuestion({ callId: "q1", open: true }, [waitingApproval])).toBeUndefined();
  });

  it("names what an allow gives the call, a full-rights run, the reviewer and the facts", () => {
    const sheet = approvalSheet(
      {
        ...PUSH,
        grants: [
          { kind: "host", host: "github.com", port: 443 },
          { kind: "env", name: "GH_TOKEN" },
        ],
        launch: { kind: "unsandboxed" },
        judged: {
          category: "network",
          rationale: "no message of yours names this host",
          risk: "medium",
          user_authorization: "low",
          verdict: "ask_user",
        },
        facts: ["runs /usr/bin/git"],
      },
      CONTEXT,
      [PUSH],
    );
    expect(sheet.notes).toEqual([
      "an allow gives the call: github.com:443 reachable, the variable GH_TOKEN",
      "runs with your full rights: files, secrets, network",
      "reviewer: unsure; no message of yours names this host",
      "runs /usr/bin/git",
    ]);
    expect(approvalSheet(PUSH, CONTEXT, [PUSH]).notes).toEqual([]);
  });

  it("says who answered a question that another surface answered", () => {
    const sheet = approvalSheet(
      { ...PUSH, status: "denied", decision: "deny", answeredBy: "shell", closedAt: 9_000 },
      CONTEXT,
      [],
    );
    expect(sheet.closed).toEqual({ parts: ["denied in a terminal on desktop"] });
  });
});

describe("approvalCall", () => {
  it("is the tool call of a question whose call the transcript does not hold", () => {
    expect(approvalCall(PUSH)).toMatchObject({
      callId: "c1",
      tool: "shell",
      summary: "git push -u origin fix/steer-race",
      done: false,
      bytes: 0,
      startedAt: 1_000,
    });
    expect(
      approvalCall({ ...PUSH, summary: "run `systemctl restart nginx` as root" }),
    ).toMatchObject({ tool: "", summary: "run `systemctl restart nginx` as root" });
  });
});
