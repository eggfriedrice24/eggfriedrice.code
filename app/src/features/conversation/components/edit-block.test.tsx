import { describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";

import type { ToolCallView } from "@/lib/stream/reducer";

import { EditBlockView } from "./edit-block";

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

const CALL: ToolCallView = {
  callId: "c1",
  tool: "apply_patch",
  summary: "*** Begin Patch",
  tail: "",
  bytes: 0,
  done: true,
  isError: false,
  exitCode: undefined,
  output: "Done",
  refusal: undefined,
  diff: DIFF,
  leftRunning: false,
  startedAt: undefined,
  endedAt: undefined,
};

function bigDiff(): string {
  const hunks = Array.from({ length: 12 }, (_, n) => {
    const at = String((n + 1) * 1000);
    const lines: string[] = Array.from({ length: 40 }, () => " same");
    return [`@@ -${at},41 +${at},41 @@`, "-old", "+new", ...lines].join("\n");
  });
  return hunks.join("\n");
}

describe("the collapsed edit block", () => {
  it("names the file and counts the lines in the header", async () => {
    await render(
      <EditBlockView
        call={CALL}
        diff={DIFF}
        expanded={false}
        allHunks={false}
        onToggle={jest.fn()}
        onShowAllHunks={jest.fn()}
      />,
    );
    expect(screen.getByText("edit")).toBeOnTheScreen();
    expect(screen.getByText("crates/efr-daemon/src/turn.rs")).toBeOnTheScreen();
    expect(screen.getByText("+3")).toBeOnTheScreen();
    expect(screen.getByText("−2")).toBeOnTheScreen();
    expect(
      screen.getByRole("button", {
        name: "Edit crates/efr-daemon/src/turn.rs, 3 lines added, 2 lines removed",
      }),
    ).toBeOnTheScreen();
  });

  it("previews 4 lines, each read as added or removed", async () => {
    await render(
      <EditBlockView
        call={CALL}
        diff={DIFF}
        expanded={false}
        allHunks={false}
        onToggle={jest.fn()}
        onShowAllHunks={jest.fn()}
      />,
    );
    expect(screen.getByLabelText("removed:     self.pending.lock().push(msg);")).toBeOnTheScreen();
    expect(screen.getByLabelText("added:     let state = self.state.lock();")).toBeOnTheScreen();
    expect(screen.queryByText("@@ -86,4 +86,5 @@ impl TurnLoop")).toBeNull();
    expect(screen.getByText("87")).toBeOnTheScreen();
  });

  it("expands on its button", async () => {
    const onToggle = jest.fn<() => void>();
    await render(
      <EditBlockView
        call={CALL}
        diff={DIFF}
        expanded={false}
        allHunks={false}
        onToggle={onToggle}
        onShowAllHunks={jest.fn()}
      />,
    );
    await fireEvent.press(screen.getByRole("button", { name: "show full diff · 2 hunks" }));
    expect(onToggle).toHaveBeenCalledTimes(1);
  });
});

describe("the expanded edit block", () => {
  it("shows each hunk with its header, both numbers and the changed run", async () => {
    await render(
      <EditBlockView
        call={CALL}
        diff={DIFF}
        expanded
        allHunks={false}
        onToggle={jest.fn()}
        onShowAllHunks={jest.fn()}
      />,
    );
    expect(screen.getByText("@@ -86,4 +86,5 @@ impl TurnLoop")).toBeOnTheScreen();
    expect(screen.getByText("@@ -240,3 +241,3 @@ mod tests")).toBeOnTheScreen();
    expect(screen.getByText("dropped").props.className).toContain("font-bold");
    expect(screen.getByText("queued").props.className).toContain("text-diff-highlight-added");
    expect(screen.getByRole("button", { name: "collapse" })).toBeOnTheScreen();
    const block = screen.getByRole("button", { name: /^Edit crates/ });
    expect(block.props.accessibilityState).toMatchObject({ expanded: true });
  });

  it("holds back the hunks after the third of a big diff", async () => {
    const onShowAllHunks = jest.fn<() => void>();
    await render(
      <EditBlockView
        call={CALL}
        diff={bigDiff()}
        expanded
        allHunks={false}
        onToggle={jest.fn()}
        onShowAllHunks={onShowAllHunks}
      />,
    );
    expect(screen.getAllByText(/^@@ /)).toHaveLength(3);
    await fireEvent.press(screen.getByRole("button", { name: "show remaining 9 hunks" }));
    expect(onShowAllHunks).toHaveBeenCalledTimes(1);
  });

  it("shows every hunk once asked", async () => {
    await render(
      <EditBlockView
        call={CALL}
        diff={bigDiff()}
        expanded
        allHunks
        onToggle={jest.fn()}
        onShowAllHunks={jest.fn()}
      />,
    );
    expect(screen.getAllByText(/^@@ /)).toHaveLength(12);
    expect(screen.queryByRole("button", { name: /show remaining/ })).toBeNull();
  });
});
