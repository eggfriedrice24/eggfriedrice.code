import { describe, expect, it } from "vitest";

import type { ToolCallView, WebStepView } from "@/lib/stream/reducer";
import {
  errorSpans,
  lineCount,
  MAX_OUTPUT_LINES,
  moreLabel,
  toolHeader,
  toolLabel,
  toolOutput,
  toolStatus,
  webPage,
  webReason,
  writingBlock,
} from "./tool-block";

function call(overrides: Partial<ToolCallView> = {}): ToolCallView {
  return {
    callId: "c1",
    tool: "shell",
    summary: "cargo test -p efr-daemon",
    tail: "",
    bytes: 0,
    done: false,
    isError: false,
    exitCode: undefined,
    output: undefined,
    refusal: undefined,
    diff: undefined,
    leftRunning: false,
    startedAt: 1_000,
    endedAt: undefined,
    ...overrides,
  };
}

describe("toolHeader", () => {
  it("names a shell command with $ and lets it wrap", () => {
    expect(toolHeader(call())).toEqual({
      verb: "$",
      subject: "cargo test -p efr-daemon",
      wraps: true,
    });
  });

  it("names a read and a file tool, cut in the middle", () => {
    expect(toolHeader(call({ tool: "read_file", summary: "crates/a.rs" }))).toEqual({
      verb: "read",
      subject: "crates/a.rs",
      wraps: false,
    });
    expect(toolHeader(call({ tool: "apply_patch", summary: "crates/a.rs" })).verb).toBe("edit");
  });

  it("shows another tool by its name, without a subject that repeats it", () => {
    expect(toolHeader(call({ tool: "web_fetch", summary: "web_fetch" }))).toEqual({
      verb: "web_fetch",
      subject: "",
      wraps: false,
    });
  });
});

describe("toolStatus", () => {
  it("runs with the elapsed time from 2 s on", () => {
    expect(toolStatus(call(), false, 2_000)).toEqual({ kind: "running", elapsed: undefined });
    expect(toolStatus(call(), false, 13_500)).toEqual({ kind: "running", elapsed: "12s" });
  });

  it("waits with no time while its approval waits, then counts from the answer", () => {
    expect(toolStatus(call(), false, 60_000, { pending: true, closedAt: undefined })).toEqual({
      kind: "waiting",
    });
    const answered = { pending: false, closedAt: 50_000 };
    expect(toolStatus(call(), false, 51_000, answered)).toEqual({
      kind: "running",
      elapsed: undefined,
    });
    expect(toolStatus(call(), false, 62_500, answered)).toEqual({
      kind: "running",
      elapsed: "12s",
    });
    const done = call({ done: true, exitCode: 0, endedAt: 53_000 });
    expect(toolStatus(done, false, 0, answered)).toEqual({ kind: "done", detail: "0 · 3s" });
  });

  it("gives the exit code and the time when done", () => {
    const done = call({ done: true, exitCode: 0, endedAt: 39_000 });
    expect(toolStatus(done, false, 0)).toEqual({ kind: "done", detail: "0 · 38s" });
  });

  it("fails with the exit code and the time", () => {
    const failed = call({ done: true, isError: true, exitCode: 101, endedAt: 10_000 });
    expect(toolStatus(failed, false, 0)).toEqual({ kind: "failed", detail: "101 · 9s" });
  });

  it("fails a call that its ended turn left without an answer", () => {
    expect(toolStatus(call(), true, 0)).toEqual({ kind: "failed", detail: "stopped" });
    expect(toolStatus(call({ exitCode: 130 }), true, 0)).toEqual({
      kind: "failed",
      detail: "130",
    });
  });

  it("says that efr refused a call", () => {
    const refused = call({ done: true, isError: true, refusal: "the sandbox" });
    expect(toolStatus(refused, false, 0)).toEqual({ kind: "failed", detail: "refused" });
  });

  it("counts the lines of a read", () => {
    const read = call({ tool: "read_file", done: true, output: "a\nb\nc\n", endedAt: 2_000 });
    expect(toolStatus(read, false, 0)).toEqual({ kind: "done", detail: "3 lines" });
  });

  it("always has a number or a word next to the glyph", () => {
    expect(toolStatus(call({ done: true, startedAt: undefined }), false, 0)).toEqual({
      kind: "done",
      detail: "done",
    });
  });
});

describe("toolOutput", () => {
  const lines = Array.from({ length: 14 }, (_, i) => `line ${String(i + 1)}`).join("\n");

  it("shows the last 3 lines of the tail while collapsed", () => {
    const output = toolOutput(call({ tail: lines, bytes: lines.length }), false);
    expect(output.lines.map((line) => line.text)).toEqual(["line 12", "line 13", "line 14"]);
    expect(output.total).toBe(14);
    expect(moreLabel(output, false)).toBe("show all 14 lines");
  });

  it("shows the whole output when expanded, at most 200 lines", () => {
    const long = Array.from({ length: 250 }, (_, i) => String(i)).join("\n");
    const output = toolOutput(call({ done: true, output: long, tail: "" }), true);
    expect(output.lines).toHaveLength(MAX_OUTPUT_LINES);
    expect(output.capped).toBe(true);
    expect(moreLabel(output, true)).toBe("collapse");
  });

  it("keeps the key of a line while the output grows", () => {
    const before = toolOutput(call({ tail: "a\nb\nc", bytes: 5 }), false);
    const after = toolOutput(call({ tail: "a\nb\nc\nd", bytes: 7 }), false);
    const keyOf = (output: typeof before, text: string) =>
      output.lines.find((line) => line.text === text)?.key;
    expect(keyOf(after, "b")).toBe(keyOf(before, "b"));
    expect(keyOf(after, "c")).toBe(keyOf(before, "c"));
  });

  it("keeps the keys when efrd drops the start of a bounded tail", () => {
    const before = toolOutput(call({ tail: "a\nb\nc", bytes: 105 }), false);
    const after = toolOutput(call({ tail: "b\nc\nd", bytes: 107 }), false);
    expect(after.lines[1]?.key).toBe(before.lines[2]?.key);
  });

  it("adds a refusal and a command that still runs as quiet lines", () => {
    const output = toolOutput(
      call({ done: true, refusal: "efr refused: the path is a secret", leftRunning: true }),
      false,
    );
    expect(output.lines.map((line) => line.text)).toEqual([
      "efr refused: the path is a secret",
      "still runs in the shell",
    ]);
  });

  it("shows no output for a read", () => {
    expect(toolOutput(call({ tool: "read_file", tail: "x" }), true).lines).toEqual([]);
  });

  it("has no button when every line shows", () => {
    expect(moreLabel(toolOutput(call({ tail: "a\nb" }), false), false)).toBeUndefined();
  });
});

describe("errorSpans", () => {
  it("marks each error word", () => {
    expect(errorSpans("error: could not compile, Error[E1]")).toEqual([
      { start: 0, text: "error", error: true },
      { start: 5, text: ": could not compile, ", error: false },
      { start: 26, text: "Error", error: true },
      { start: 31, text: "[E1]", error: false },
    ]);
    expect(errorSpans("terror")).toEqual([{ start: 0, text: "terror", error: false }]);
    expect(errorSpans("")).toEqual([{ start: 0, text: "", error: false }]);
  });
});

describe("the small texts", () => {
  it("counts lines", () => {
    expect(lineCount("a")).toBe("1 line");
    expect(lineCount("a\nb\n")).toBe("2 lines");
  });

  it("names a patch that the model writes, with its size", () => {
    expect(writingBlock({ tool: "apply_patch", bytes: 2_345 })).toEqual({
      verb: "writing patch",
      size: "2.3 KB",
    });
    expect(writingBlock({ tool: "write_file", bytes: 12 }).verb).toBe("writing write_file");
  });

  it("labels a block for VoiceOver", () => {
    const header = toolHeader(call());
    expect(toolLabel(header, { kind: "done", detail: "0 · 38s" })).toBe(
      "Command cargo test -p efr-daemon, done, 0 · 38s",
    );
    expect(toolLabel(header, { kind: "running", elapsed: undefined })).toBe(
      "Command cargo test -p efr-daemon, running",
    );
  });
});

/** A step of web search as its tool block holds it. */
function web(step: Partial<WebStepView>, overrides: Partial<ToolCallView> = {}): ToolCallView {
  return call({
    tool: "web",
    summary: "",
    web: {
      action: "search",
      query: undefined,
      url: undefined,
      results: undefined,
      error: undefined,
      ...step,
    },
    ...overrides,
  });
}

describe("a step of web search", () => {
  it("names a search by its query and an open by its page, as TM-Web does", () => {
    expect(toolHeader(web({ query: '"ROB RUN" areweanticheatyet' }))).toEqual({
      verb: "web",
      subject: '"ROB RUN" areweanticheatyet',
      wraps: false,
    });
    const open = web({ action: "open", url: "https://www.protondb.com/app/2384710" });
    expect(toolHeader(open).subject).toBe("open protondb.com/app/2384710");
    expect(toolHeader(web({})).subject).toBe("search");
    expect(webPage("http://areweanticheatyet.com/")).toBe("areweanticheatyet.com");
  });

  it("says the count of results and the time, or the time alone", () => {
    const done = { done: true, startedAt: 1_000, endedAt: 7_000 };
    expect(toolStatus(web({ results: 10 }, done), false, 0)).toEqual({
      kind: "done",
      detail: "10 results · 6s",
    });
    expect(toolStatus(web({ results: 0 }, done), false, 0).kind).toBe("done");
    expect(toolStatus(web({ results: 0 }, done), false, 0)).toMatchObject({
      detail: "0 results · 6s",
    });
    expect(toolStatus(web({}, { ...done, endedAt: 6_000 }), false, 0)).toEqual({
      kind: "done",
      detail: "5s",
    });
  });

  it("says why a step failed in the words of the terminal", () => {
    const failed = web(
      { error: "too_many_requests" },
      { done: true, isError: true, startedAt: 1_000, endedAt: 2_000 },
    );
    expect(toolStatus(failed, false, 0)).toEqual({
      kind: "failed",
      detail: "too many requests · 1s",
    });
    expect(webReason("search", "limit_reached")).toBe("search limit of this turn reached");
    expect(webReason("open", "limit_reached")).toBe("page limit of this turn reached");
    expect(webReason("search", "unavailable")).toBe("web search not available");
    // A step that its turn left open did not finish.
    expect(toolStatus(web({}), true, 0)).toEqual({ kind: "failed", detail: "did not finish" });
    // While it runs, its time counts from its start.
    expect(toolStatus(web({}), false, 4_000)).toEqual({ kind: "running", elapsed: "3s" });
  });
});
