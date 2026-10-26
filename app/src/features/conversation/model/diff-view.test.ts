import { describe, expect, it } from "vitest";

import {
  BIG_DIFF_LINES,
  diffLabel,
  diffModel,
  expandLabel,
  lineLabel,
  previewNumber,
  previewRows,
  remainingLabel,
  shownHunks,
  statTexts,
} from "./diff-view";

const DIFF = [
  "--- a/crates/efr-daemon/src/turn.rs",
  "+++ b/crates/efr-daemon/src/turn.rs",
  "@@ -86,4 +86,5 @@ impl TurnLoop",
  " /// Hand a message to the running turn.",
  " pub fn steer(&self, msg: Message) -> Steer {",
  "-    self.pending.lock().push(msg);",
  "+    let state = self.state.lock();",
  "+    self.pending.lock().push(msg);",
  " }",
  "@@ -240,3 +241,3 @@ mod tests",
  "-fn steer_after_finish_is_dropped() {",
  "+fn steer_after_finish_is_queued() {",
  " let mut turn = TurnLoop::test();",
].join("\n");

describe("diffModel", () => {
  const model = diffModel(DIFF, "fallback");

  it("reads the path and the counts", () => {
    expect(model.path).toBe("crates/efr-daemon/src/turn.rs");
    expect(model.newFile).toBe(false);
    expect(statTexts(model)).toEqual({ added: "+3", removed: "−2" });
    expect(diffLabel(model)).toBe(
      "Edit crates/efr-daemon/src/turn.rs, 3 lines added, 2 lines removed",
    );
  });

  it("splits the lines into hunks with their headers", () => {
    expect(model.hunks.map((hunk) => hunk.header)).toEqual([
      "@@ -86,4 +86,5 @@ impl TurnLoop",
      "@@ -240,3 +241,3 @@ mod tests",
    ]);
    expect(model.lineCount).toBe(9);
    expect(expandLabel(model)).toBe("show full diff · 2 hunks");
  });

  it("marks the changed run of a pair, without the shared start and end", () => {
    const rows = model.hunks[1]?.rows ?? [];
    expect(rows[0]?.spans).toEqual([
      { start: 0, text: "-fn steer_after_finish_is_", changed: false },
      { start: 26, text: "dropped", changed: true },
      { start: 33, text: "() {", changed: false },
    ]);
    expect(rows[1]?.spans).toEqual([
      { start: 0, text: "+fn steer_after_finish_is_", changed: false },
      { start: 26, text: "queued", changed: true },
      { start: 32, text: "() {", changed: false },
    ]);
    expect(rows.map((row) => row.id)).toEqual([6, 7, 8]);
  });

  it("marks nothing when a pair has nothing in common", () => {
    const rows = diffModel("@@ -1 +1 @@\n-abc\n+xyz\n", "f").hunks[0]?.rows ?? [];
    expect(rows.map((row) => row.spans.length)).toEqual([1, 1]);
  });

  it("knows a new file and uses the fallback path without headers", () => {
    const created = diffModel("--- /dev/null\n+++ b/notes.md\n@@ -0,0 +1 @@\n+hi\n", "x");
    expect(created).toMatchObject({ path: "notes.md", newFile: true });
    expect(expandLabel(created)).toBe("show full diff · new file");
    expect(diffModel("@@ -1 +1 @@\n-a\n+b\n", "~/.zshrc").path).toBe("~/.zshrc");
    expect(expandLabel(diffModel("@@ -1 +1 @@\n-a\n+b\n", "x"))).toBe("show full diff · 1 hunk");
  });
});

describe("previewRows", () => {
  it("starts one line of context before the first change", () => {
    const rows = previewRows(diffModel(DIFF, "f"));
    expect(rows.map((row) => row.line.text)).toEqual([
      " pub fn steer(&self, msg: Message) -> Steer {",
      "-    self.pending.lock().push(msg);",
      "+    let state = self.state.lock();",
      "+    self.pending.lock().push(msg);",
    ]);
    expect(rows.map((row) => previewNumber(row.line))).toEqual([87, 88, 88, 89]);
  });
});

/** A hunk of 100 context lines at line `n` * 1000. */
function contextHunk(n: number): string {
  const lines: string[] = Array.from({ length: 100 }, () => " x");
  return [`@@ -${String(n * 1000)},100 +${String(n * 1000)},100 @@`, ...lines].join("\n");
}

describe("shownHunks", () => {
  it("shows every hunk of a small diff", () => {
    const model = diffModel(DIFF, "f");
    expect(shownHunks(model, false)).toEqual({ hunks: model.hunks, hidden: 0 });
  });

  it("holds back the hunks after the third of a big diff until asked", () => {
    const big = diffModel(Array.from({ length: 12 }, (_, n) => contextHunk(n + 1)).join("\n"), "f");
    expect(big.lineCount).toBeGreaterThan(BIG_DIFF_LINES);
    expect(shownHunks(big, false).hunks).toHaveLength(3);
    expect(shownHunks(big, false).hidden).toBe(9);
    expect(remainingLabel(9)).toBe("show remaining 9 hunks");
    expect(shownHunks(big, true).hidden).toBe(0);
  });
});

describe("lineLabel", () => {
  it("says added or removed for VoiceOver", () => {
    expect(lineLabel({ kind: "added", text: "+x" })).toBe("added: x");
    expect(lineLabel({ kind: "removed", text: "-y" })).toBe("removed: y");
    expect(lineLabel({ kind: "context", text: " z" })).toBe(" z");
  });
});
