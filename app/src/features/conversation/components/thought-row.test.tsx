import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { act, fireEvent, render, screen, within } from "@testing-library/react-native";

import { draftItem, eventItem, promptQueued, run, turnStarted } from "@/lib/stream/testing";

import { ThoughtRow, ThoughtRowView } from "./thought-row";
import { InScope, testScope } from "./transcript-testing";

const HIDDEN = { includeHiddenElements: true } as const;
const NOW = new Date(2026, 9, 26, 14, 0, 0).getTime();
const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";

beforeEach(() => {
  jest.useFakeTimers({ now: NOW });
});

afterEach(() => {
  jest.useRealTimers();
});

describe("the thought row", () => {
  it("is one collapsed line with a chevron", async () => {
    await render(
      <ThoughtRowView
        title="Planning the migration"
        text="412 JSON files."
        live={false}
        startedAt={NOW}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    const row = screen.getByRole("button", { name: "thought, Planning the migration" });
    expect(screen.getByText("Planning the migration").props.className).toContain("text-text-muted");
    expect(row.props.accessibilityState).toMatchObject({ expanded: false });
    expect(screen.queryByText("412 JSON files.")).toBeNull();
  });

  it("shows the reasoning and the loader when expanded and live", async () => {
    await render(
      <ThoughtRowView
        title="Planning the migration"
        text={"412 JSON files.\n\nSQLite with WAL."}
        live
        startedAt={NOW}
        expanded
        onToggle={jest.fn()}
      />,
    );
    await act(async () => {
      jest.advanceTimersByTime(12_000);
    });
    expect(
      screen.getByRole("button", { name: "thinking, Planning the migration, 12s" }),
    ).toBeOnTheScreen();
    expect(screen.getByText("· 12s")).toBeOnTheScreen();
    expect(screen.getByText("412 JSON files.")).toBeOnTheScreen();
    expect(screen.getByText("SQLite with WAL.")).toBeOnTheScreen();
    expect(screen.getAllByTestId("braille-cell", HIDDEN)).toHaveLength(1);
    // Only the line is the button, so VoiceOver reads the reasoning on its own.
    const line = screen.getByRole("button", { name: /^thinking/ });
    expect(within(line).queryByText("412 JSON files.")).toBeNull();
  });

  it("takes the first words as an italic title when the reasoning has none", async () => {
    const text =
      "The steer arrives on its own channel, so if the turn loop checks it only between tool calls";
    await render(
      <ThoughtRowView
        title={undefined}
        text={text}
        live={false}
        startedAt={NOW}
        endedAt={NOW + 9_000}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText(text).props.className).toContain("italic");
    expect(screen.getByText("· 9s")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: `thought, ${text}, 9s` })).toBeOnTheScreen();
  });

  it("says thought once the first thought ended, though the turn still runs", async () => {
    await render(
      <ThoughtRowView
        title="Planning the migration"
        text="412 JSON files."
        live
        startedAt={NOW}
        endedAt={NOW + 9_000}
        expanded={false}
        onToggle={jest.fn()}
      />,
    );
    expect(
      screen.getByRole("button", { name: "thought, Planning the migration, 9s" }),
    ).toBeOnTheScreen();
    expect(screen.queryByTestId("braille-cell", HIDDEN)).toBeNull();
  });

  it("says the reasoning is gone when efrd kept only the title", async () => {
    await render(
      <ThoughtRowView
        title="Planning the migration"
        text=""
        live={false}
        startedAt={NOW}
        expanded
        onToggle={jest.fn()}
      />,
    );
    expect(screen.getByText("the reasoning isn't kept after the turn ends")).toBeOnTheScreen();
  });

  it("keeps its open state in the row state of the conversation", async () => {
    const state = run([
      eventItem(1, promptQueued(TURN, "Go.")),
      eventItem(2, turnStarted(TURN)),
      draftItem(TURN, 2, {
        kind: "reasoning",
        offset: 0,
        delta: "Reading the loop.",
        title: "Reading",
      }),
    ]);
    const { scope } = testScope(state);
    await render(
      <InScope scope={scope}>
        <ThoughtRow rowKey={`r:${TURN}`} turnId={TURN} />
      </InScope>,
    );
    expect(screen.queryByText("Reading the loop.")).toBeNull();
    await fireEvent.press(screen.getByRole("button", { name: /thinking, Reading/ }));
    expect(screen.getByText("Reading the loop.")).toBeOnTheScreen();
    expect(scope.rowUi.store.get().get(`r:${TURN}`)?.has("expanded")).toBe(true);
  });
});
