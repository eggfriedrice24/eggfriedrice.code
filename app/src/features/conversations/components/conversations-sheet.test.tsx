import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { act, fireEvent, render, screen, within } from "@testing-library/react-native";
import type { ReactNode } from "react";

import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import type {
  ConversationStatus,
  ConversationsSearch,
  ConversationsSearchResult,
} from "@/lib/protocol/generated";
import type { ConversationAddress } from "@/lib/query/keys";
import { createQueryClient } from "@/lib/query/query-client";
import { FakeCaller } from "@/lib/query/testing";

import { SEARCH_LIMIT } from "../conversations-queries";
import { october } from "../model/testing";
import { SEARCH_DEBOUNCE_MS } from "../model/sheet-view";
import { ConversationsSheet } from "./conversations-sheet";
import { ROW_GLYPHS } from "./sheet-row";

jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const NOW = october(26, 14);
const MINUTE = 60_000;
const MACHINES = [
  { id: "d", name: "desktop" },
  { id: "l", name: "laptop" },
];

interface Conversation {
  readonly id: string;
  readonly title: string;
  readonly at: number;
  readonly cwd?: string;
  readonly status?: ConversationStatus;
}

function result(conversations: readonly Conversation[]): ConversationsSearchResult {
  return {
    rows: conversations.map((conversation) => ({
      conversation: {
        summary: {
          id: conversation.id,
          title: conversation.title,
          status: conversation.status ?? "idle",
          cwd: conversation.cwd ?? null,
          created_at: new Date(conversation.at - MINUTE).toISOString(),
          updated_at: new Date(conversation.at).toISOString(),
          last_seq: 1,
        },
        route: { kind: "outside" },
        turns: 1,
      },
      short_id: conversation.id,
    })),
  };
}

// The sample of board R-AllSheet, in part.
const RECENT: Readonly<Record<string, readonly Conversation[]>> = {
  d: [
    {
      id: "steer",
      title: "Fix steer race in turn loop",
      at: NOW - 20 * MINUTE,
      cwd: "/home/me/code/efr",
      status: "awaiting_approval",
    },
    { id: "fan", title: "Why is the fan loud", at: NOW - 2 * MINUTE, cwd: "/home/me" },
    {
      id: "migrate",
      title: "Migrate session store to SQLite",
      at: NOW - 3 * MINUTE,
      cwd: "/home/me/code/efr",
      status: "running",
    },
    { id: "docker", title: "Clean up old docker images", at: october(19, 9), cwd: "/home/me" },
  ],
  l: [
    {
      id: "btrfs",
      title: "Prune old btrfs snapshots",
      at: NOW - 30 * MINUTE,
      cwd: "/home/me",
      status: "awaiting_approval",
    },
    { id: "restic", title: "Set up restic backups", at: october(25, 20), cwd: "/home/me" },
  ],
};

/** A caller whose empty search gives RECENT, and whose search gives `hits`. */
function sampleCaller(
  hits: (query: string, machine: string) => readonly Conversation[] = () => [],
): FakeCaller {
  return new FakeCaller({
    "conversations.search": (params: ConversationsSearch, machine: string) =>
      result(params.query === "" ? (RECENT[machine] ?? []) : hits(params.query, machine)),
  });
}

let client: QueryClient;

function wrapper(props: { readonly children: ReactNode }) {
  return <QueryClientProvider client={client}>{props.children}</QueryClientProvider>;
}

interface Options {
  readonly caller?: FakeCaller;
  readonly machines?: typeof MACHINES;
  readonly onOpen?: (ref: ConversationAddress) => void;
  readonly onNew?: () => void;
}

async function renderSheet(options: Options = {}): Promise<FakeCaller> {
  const caller = options.caller ?? sampleCaller();
  await render(
    <ConversationsSheet
      caller={caller}
      machines={options.machines ?? MACHINES}
      now={NOW}
      onOpen={options.onOpen ?? (() => undefined)}
      onNew={options.onNew ?? (() => undefined)}
    />,
    { wrapper },
  );
  await flush();
  return caller;
}

/** Lets the answers of the fake caller and the notifications of Query arrive. */
async function flush(ms = 0): Promise<void> {
  await act(async () => {
    await jest.advanceTimersByTimeAsync(ms);
  });
  await act(async () => {
    await jest.advanceTimersByTimeAsync(0);
  });
}

async function type(text: string): Promise<void> {
  await fireEvent.changeText(screen.getByLabelText("Search conversations"), text);
}

function rowTitles(pane: string): string[] {
  return within(screen.getByTestId(pane))
    .getAllByTestId("sheet-row")
    .map((row) => String(row.props.accessibilityLabel).split(", ")[0] ?? "");
}

beforeEach(() => {
  jest.useFakeTimers();
  client = createQueryClient();
  jest.mocked(copyText).mockClear();
  jest.mocked(haptic).mockClear();
});

afterEach(() => {
  client.clear();
  jest.useRealTimers();
});

describe("the sections of the sheet", () => {
  it("puts the waiting ones of every machine on top, then today and older", async () => {
    const caller = await renderSheet();
    expect(caller.callsOf("conversations.search").map((call) => call.machine)).toEqual(["d", "l"]);
    expect(screen.getByText("waiting · 2")).toBeOnTheScreen();
    expect(screen.getByText("today")).toBeOnTheScreen();
    expect(screen.getByText("this week")).toBeOnTheScreen();
    expect(screen.getByText("older")).toBeOnTheScreen();
    expect(rowTitles("sheet-sections")).toEqual([
      "Fix steer race in turn loop",
      "Prune old btrfs snapshots",
      "Why is the fan loud",
      "Migrate session store to SQLite",
      "Set up restic backups",
      "Clean up old docker images",
    ]);
  });

  it("draws a row with its status word, machine, cwd and time", async () => {
    await renderSheet();
    const waiting = screen.getByLabelText(
      "Fix steer race in turn loop, needs approval · desktop · ~/code/efr",
    );
    expect(waiting).toBeOnTheScreen();
    expect(within(waiting).getByText("needs approval")).toBeOnTheScreen();
    expect(screen.getByLabelText("Why is the fan loud, desktop · ~ · 2m")).toBeOnTheScreen();
    expect(
      screen.getByLabelText("Set up restic backups, laptop · ~ · yesterday"),
    ).toBeOnTheScreen();
  });

  it("gives each row an SF Symbol of its status, yolk for waiting and running", async () => {
    await renderSheet();
    const hidden = { includeHiddenElements: true } as const;
    expect(screen.getAllByTestId("row-glyph-approval", hidden)).toHaveLength(2);
    expect(screen.getAllByTestId("row-glyph-running", hidden)).toHaveLength(1);
    expect(screen.getAllByTestId("row-glyph-none", hidden).length).toBeGreaterThan(0);
    const migrate = screen.getByLabelText(
      "Migrate session store to SQLite, running · desktop · ~/code/efr",
    );
    expect(within(migrate).getByLabelText("Running")).toBeOnTheScreen();
    expect(ROW_GLYPHS.approval).toEqual({
      name: "exclamationmark.circle.fill",
      tone: "accent_primary",
      label: "Needs approval",
    });
    expect(ROW_GLYPHS.running.tone).toBe("accent_primary");
    expect(ROW_GLYPHS.none.tone).toBe("text_subtle");
  });

  it("has New conversation on top, before the search field", async () => {
    const onNew = jest.fn<() => void>();
    await renderSheet({ onNew });
    const tops = screen.getAllByRole(/button|search/).map((node) => node.props.accessibilityLabel);
    expect(tops.slice(0, 2)).toEqual(["New conversation", "Search conversations"]);
    await fireEvent.press(screen.getByRole("button", { name: "New conversation" }));
    expect(onNew).toHaveBeenCalledTimes(1);
  });

  it("shows the loading line while no machine has answered", async () => {
    const caller = new FakeCaller({
      "conversations.search": () => new Promise<ConversationsSearchResult>(() => undefined),
    });
    await renderSheet({ caller });
    expect(screen.getByLabelText("loading conversations")).toBeOnTheScreen();
    expect(screen.queryAllByTestId("sheet-row")).toHaveLength(0);
  });

  it("says so when there are no conversations, or no machine", async () => {
    await renderSheet({ caller: new FakeCaller({ "conversations.search": () => result([]) }) });
    expect(screen.getByText("no conversations yet")).toBeOnTheScreen();
    client.clear();
    await renderSheet({ machines: [] });
    expect(screen.getByText("no machine is paired")).toBeOnTheScreen();
  });

  it("shows the rows of the machines that answered and counts the others", async () => {
    const caller = new FakeCaller({
      "conversations.search": (params: ConversationsSearch, machine: string) => {
        if (machine === "l") {
          throw new Error("laptop is gone");
        }
        return result(params.query === "" ? (RECENT[machine] ?? []) : []);
      },
    });
    await renderSheet({ caller });
    expect(rowTitles("sheet-sections")).toHaveLength(4);
    expect(screen.getByText("1 machine did not answer")).toBeOnTheScreen();
  });
});

describe("the search of the sheet", () => {
  it("filters the loaded rows at once by machine, before efrd answers", async () => {
    const caller = await renderSheet();
    await type("laptop");
    expect(screen.getByText("results · 2")).toBeOnTheScreen();
    expect(rowTitles("sheet-results")).toEqual([
      "Prune old btrfs snapshots",
      "Set up restic backups",
    ]);
    expect(screen.getByLabelText("searching")).toBeOnTheScreen();
    // The search of efrd waits for the debounce.
    expect(caller.callsOf("conversations.search")).toHaveLength(2);
  });

  it("asks every machine after 200 ms and puts the hits of efrd first", async () => {
    const caller = await renderSheet({
      caller: sampleCaller((query, machine) =>
        query === "fan" && machine === "d"
          ? [{ id: "curve", title: "Tune the curve", at: october(10, 9) }]
          : [],
      ),
    });
    await type("fan");
    await flush(SEARCH_DEBOUNCE_MS - 1);
    expect(caller.callsOf("conversations.search")).toHaveLength(2);
    await flush(1);
    const searches = caller.callsOf("conversations.search").slice(2);
    expect(searches.map((call) => [call.machine, call.params])).toEqual([
      ["d", { query: "fan", limit: SEARCH_LIMIT }],
      ["l", { query: "fan", limit: SEARCH_LIMIT }],
    ]);
    expect(rowTitles("sheet-results")).toEqual(["Tune the curve", "Why is the fan loud"]);
    expect(screen.queryByLabelText("searching")).toBeNull();
  });

  it("asks efrd once for a word typed in quick keys", async () => {
    const caller = await renderSheet();
    await type("f");
    await flush(100);
    await type("fa");
    await flush(100);
    await type("fan");
    await flush(SEARCH_DEBOUNCE_MS);
    expect(caller.callsOf("conversations.search").slice(2)).toEqual([
      expect.objectContaining({ machine: "d", params: { query: "fan", limit: SEARCH_LIMIT } }),
      expect.objectContaining({ machine: "l", params: { query: "fan", limit: SEARCH_LIMIT } }),
    ]);
  });

  it("says so when nothing matches", async () => {
    await renderSheet();
    await type("kernel");
    await flush(SEARCH_DEBOUNCE_MS);
    expect(screen.getByText("no conversations match")).toBeOnTheScreen();
    expect(screen.queryByText(/^results/)).toBeNull();
  });

  it("shows the sections again when the field is cleared", async () => {
    await renderSheet();
    await type("laptop");
    expect(screen.queryByTestId("sheet-sections")).toBeNull();
    await type("");
    expect(screen.getByText("waiting · 2")).toBeOnTheScreen();
    expect(screen.queryByTestId("sheet-results")).toBeNull();
  });
});

describe("a row of the sheet", () => {
  it("opens its conversation, with its machine", async () => {
    const onOpen = jest.fn<(ref: ConversationAddress) => void>();
    await renderSheet({ onOpen });
    await fireEvent.press(screen.getByLabelText("Set up restic backups, laptop · ~ · yesterday"));
    expect(onOpen).toHaveBeenCalledWith({ machine: "l", id: "restic" });
  });

  it("opens its menu on a long press, and copies the title or the id", async () => {
    await renderSheet();
    const native = screen
      .getAllByTestId("menu-view")
      .find((element) => element.props.title === "Why is the fan loud");
    if (native === undefined) {
      throw new Error("the row of the fan has no menu");
    }
    expect(native.props.shouldOpenOnLongPress).toBe(true);
    expect(native.props.actions.map((action: { title: string }) => action.title)).toEqual([
      "Copy title",
      "Copy conversation ID",
    ]);
    await fireEvent(native, "pressAction", { nativeEvent: { event: "copy-title" } });
    await fireEvent(native, "pressAction", { nativeEvent: { event: "copy-id" } });
    expect(jest.mocked(copyText).mock.calls).toEqual([["Why is the fan loud"], ["fan"]]);
    expect(haptic).toHaveBeenCalledWith("light");
  });

  it("gives VoiceOver the two copy actions", async () => {
    await renderSheet();
    const row = screen.getByLabelText("Why is the fan loud, desktop · ~ · 2m");
    expect(row.props.accessibilityActions).toEqual([
      { name: "copy-title", label: "Copy title" },
      { name: "copy-id", label: "Copy conversation ID" },
    ]);
    await fireEvent(row, "accessibilityAction", { nativeEvent: { actionName: "copy-id" } });
    expect(copyText).toHaveBeenCalledWith("fan");
  });
});
