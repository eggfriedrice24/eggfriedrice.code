// The cases of crates/efr-render/src/code/diff/tests.rs, so the phone and the terminal
// tell the lines of a diff apart in the same way.

import { describe, expect, it } from "vitest";

import { diffLines, diffStat } from "./diff_lines";

const kinds = (lines: readonly string[]) => diffLines(lines.join("\n")).map((line) => line.kind);

describe("diffLines", () => {
  it("classifies a git diff line by line", () => {
    expect(
      kinds([
        "diff --git a/src/lib.rs b/src/lib.rs",
        "index 1111111..2222222 100644",
        "--- a/src/lib.rs",
        "+++ b/src/lib.rs",
        "@@ -1,3 +1,3 @@ fn main() {",
        " let a = 1;",
        "-let b = 2;",
        "+let b = 3;",
        "\\ No newline at end of file",
        " let c = 4;",
      ]),
    ).toEqual([
      "meta",
      "meta",
      "file",
      "file",
      "hunk",
      "context",
      "removed",
      "added",
      "note",
      "context",
    ]);
  });

  it("treats dashes and pluses inside a counted hunk as changes", () => {
    expect(
      kinds(["@@ -1,1 +1,1 @@", "--- a/looks-like-a-header", "+++ b/also", "--- a/next.rs"]),
    ).toEqual(["hunk", "removed", "added", "file"]);
  });

  it("ends an uncounted hunk at a file header", () => {
    expect(kinds(["@@", "-old", "+new", "--- a/next.rs", "+++ b/next.rs", "@@", "+x"])).toEqual([
      "hunk",
      "removed",
      "added",
      "file",
      "file",
      "hunk",
      "added",
    ]);
  });

  it("shows additions and removals outside a hunk", () => {
    expect(kinds(["+added", "-removed", "plain"])).toEqual(["added", "removed", "other"]);
  });

  it("ends a hunk at a line that cannot be in it", () => {
    expect(kinds(["@@", "+a", "prose after", "+b"])).toEqual(["hunk", "added", "other", "added"]);
  });

  it("numbers the lines of a counted hunk, with a missing count as 1", () => {
    const lines = diffLines("@@ -10,3 +10,4 @@\n a\n-b\n+c\n+d\n e\n@@ -5 +7 @@\n-x\n+y\n");
    expect(lines.map((line) => [line.oldLine, line.newLine])).toEqual([
      [undefined, undefined],
      [10, 10],
      [11, undefined],
      [undefined, 11],
      [undefined, 12],
      [12, 13],
      [undefined, undefined],
      [5, undefined],
      [undefined, 7],
    ]);
    expect(diffStat(lines)).toEqual({ added: 3, removed: 2 });
  });

  it("gives no line numbers for a header without counts", () => {
    const lines = diffLines("@@ garbage @@\n+a\n");
    expect(lines[1]).toEqual({ kind: "added", text: "+a" });
  });

  it("drops the carriage return of a CRLF line and gives nothing for an empty diff", () => {
    expect(diffLines("@@ -1 +1 @@\r\n-a\r\n+b\r\n").map((line) => line.text)).toEqual([
      "@@ -1 +1 @@",
      "-a",
      "+b",
    ]);
    expect(diffLines("")).toEqual([]);
  });
});
