import { describe, expect, it } from "vitest";

import { FakeClock } from "@/lib/net/testing";
import {
  clockTime,
  debugString,
  expiryLabel,
  MINUS,
  nextExpiryChange,
  reasonSpans,
  subjectLine,
  summaryParts,
  TIME_UP,
  WAKE_DETAIL,
  watchExpiry,
} from "./approval-block-text";

describe("summaryParts", () => {
  it("takes the command of a shell question, with the escapes of Rust's Debug form", () => {
    expect(summaryParts('shell: run "git push -u origin fix/steer-race"', undefined)).toEqual({
      subject: { kind: "command", line: "git push -u origin fix/steer-race" },
      details: [],
    });
    const quoted = summaryParts(
      String.raw`shell: run "echo \"a; b\" \\ done\t\u{1f95a}"`,
      undefined,
    );
    expect(quoted.subject).toEqual({ kind: "command", line: 'echo "a; b" \\ done\t\u{1f95a}' });
  });

  it("keeps the other subjects and lines as details, in order", () => {
    const parts = summaryParts(
      'shell: run "sudo systemctl restart nginx"; input at the terminal\nasks for: sudo',
      undefined,
    );
    expect(parts).toEqual({
      subject: { kind: "command", line: "sudo systemctl restart nginx" },
      details: ["input at the terminal", "asks for: sudo"],
    });
  });

  it("puts the first line of full mode and of a job notice turn in the details", () => {
    const full = summaryParts(
      'full mode still asks for this: delete of ~ (your home directory)\nshell: run "rm -rf ~"',
      undefined,
    );
    expect(full).toEqual({
      subject: { kind: "command", line: "rm -rf ~" },
      details: ["full mode still asks for this: delete of ~ (your home directory)"],
    });
    const wake = summaryParts('(job notice turn) shell: run "make"', undefined);
    expect(wake).toEqual({ subject: { kind: "command", line: "make" }, details: [WAKE_DETAIL] });
  });

  it("makes an edit line of a write with a diff, and keeps the summary without one", () => {
    const diff = { added: 2, removed: 1 };
    expect(summaryParts("write_file: write /home/me/.zshrc (user data)", diff)).toEqual({
      subject: { kind: "edit", path: "~/.zshrc", added: 2, removed: 1 },
      details: [],
    });
    expect(summaryParts("write_file: write /home/me/.zshrc (user data)", undefined)).toEqual({
      subject: { kind: "text", text: "write_file: write /home/me/.zshrc (user data)" },
      details: [],
    });
  });

  it("shows a summary of another form as it is", () => {
    expect(summaryParts("run `journalctl --vacuum-size=500M` as root", undefined).subject).toEqual({
      kind: "text",
      text: "run `journalctl --vacuum-size=500M` as root",
    });
    expect(summaryParts('shell: run "x" as a job (watch)', undefined).subject).toEqual({
      kind: "text",
      text: 'shell: run "x" as a job (watch)',
    });
    expect(summaryParts('shell: run "never closed', undefined).subject.kind).toBe("text");
  });
});

describe("subjectLine", () => {
  it("gives the one-line form of each subject", () => {
    expect(subjectLine({ kind: "command", line: "ls" })).toBe("$ ls");
    expect(subjectLine({ kind: "edit", path: "~/.zshrc", added: 2, removed: 1 })).toBe(
      `edit ~/.zshrc +2 ${MINUS}1`,
    );
    expect(subjectLine({ kind: "text", text: "settings: change" })).toBe("settings: change");
  });
});

describe("debugString", () => {
  it("refuses a string that does not close or has an unknown escape", () => {
    expect(debugString('"abc', 0)).toBeUndefined();
    expect(debugString(String.raw`"a\qb"`, 0)).toBeUndefined();
    expect(debugString(String.raw`"\u{110000}"`, 0)).toBeUndefined();
    expect(debugString("abc", 0)).toBeUndefined();
    expect(debugString('x"ok"', 1)).toEqual({ value: "ok", end: 5 });
  });
});

describe("reasonSpans", () => {
  it("shows the mode name in mono, as the board draws `cautious`", () => {
    expect(
      reasonSpans("Pushes to a remote. cautious asks before any network access.").map((span) => [
        span.text,
        span.mono,
      ]),
    ).toEqual([
      ["Pushes to a remote. ", false],
      ["cautious", true],
      [" asks before any network access.", false],
    ]);
  });

  it("shows paths, config.toml and $SCRATCH in mono, without the end of the sentence", () => {
    const spans = reasonSpans(
      "Writes outside this conversation's folder, ~/.dotfiles. A rule in config.toml or /etc/x, and $SCRATCH.",
    );
    expect(spans.filter((span) => span.mono).map((span) => span.text)).toEqual([
      "~/.dotfiles",
      "config.toml",
      "/etc/x",
      "$SCRATCH",
    ]);
    expect(spans.map((span) => span.text).join("")).toBe(
      "Writes outside this conversation's folder, ~/.dotfiles. A rule in config.toml or /etc/x, and $SCRATCH.",
    );
  });

  it("keeps plain words plain, such as a mode word that is not the mode", () => {
    const spans = reasonSpans("The full list of read/write rules, and full mode still asks.");
    expect(spans.filter((span) => span.mono).map((span) => span.text)).toEqual(["full"]);
    expect(spans.map((span) => span.at)).toEqual([0, 39, 43]);
  });
});

describe("the times", () => {
  it("writes a local 24-hour clock time", () => {
    const time = new Date(2026, 9, 26, 9, 5).getTime();
    expect(clockTime(time)).toBe("09:05");
    expect(clockTime(new Date(2026, 9, 26, 14, 32).getTime())).toBe("14:32");
  });

  it("counts down in whole minutes, then in seconds, then says the time is up", () => {
    expect(expiryLabel(4 * 60_000 + 59_000, 0)).toBe("expires in 4m");
    expect(expiryLabel(60_000, 0)).toBe("expires in 1m");
    expect(expiryLabel(59_999, 0)).toBe("expires in 1m");
    expect(expiryLabel(59_000, 0)).toBe("expires in 59s");
    expect(expiryLabel(1, 0)).toBe("expires in 1s");
    expect(expiryLabel(0, 0)).toBe(TIME_UP);
    expect(expiryLabel(0, 5)).toBe(TIME_UP);
  });

  it("names the next time at which the text changes", () => {
    for (const [expiresAt, now] of [
      [130_000, 0],
      [120_000, 0],
      [60_500, 0],
      [59_999, 0],
      [59_000, 0],
      [1500, 0],
    ] as const) {
      const next = nextExpiryChange(expiresAt, now);
      expect(next).toBeDefined();
      if (next === undefined) {
        continue;
      }
      expect(expiryLabel(expiresAt, next - 1)).toBe(expiryLabel(expiresAt, now));
      expect(expiryLabel(expiresAt, next)).not.toBe(expiryLabel(expiresAt, now));
    }
    expect(nextExpiryChange(100, 100)).toBeUndefined();
  });

  it("watches the countdown with one timer at a time, and stops", async () => {
    const clock = new FakeClock(0);
    const seen: string[] = [];
    const stop = watchExpiry(62_000, clock, () => seen.push(expiryLabel(62_000, clock.now())));
    expect(clock.pendingTimers).toBe(1);
    await clock.advance(5000);
    expect(seen).toEqual(["expires in 59s", "expires in 58s", "expires in 57s"]);
    stop();
    await clock.advance(60_000);
    expect(seen).toHaveLength(3);
    expect(clock.pendingTimers).toBe(0);
  });

  it("stops by itself when the time is up", async () => {
    const clock = new FakeClock(0);
    const seen: string[] = [];
    watchExpiry(2000, clock, () => seen.push(expiryLabel(2000, clock.now())));
    await clock.advance(10_000);
    expect(seen).toEqual(["expires in 1s", TIME_UP]);
    expect(clock.pendingTimers).toBe(0);
  });
});
