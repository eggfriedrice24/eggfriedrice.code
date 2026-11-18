import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { act, fireEvent, render, screen, within } from "@testing-library/react-native";
import { useState, type ReactNode } from "react";
import { TextInput } from "react-native";

import { ConnectionError } from "@/lib/net/errors";
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

import { SEARCH_LIMIT, conversationsKeys } from "../conversations-queries";
import { october } from "../model/testing";
import { SEARCH_DEBOUNCE_MS } from "../model/sheet-view";
import { ConversationsSheet, type ConversationsSheetProps } from "./conversations-sheet";
import { ROW_SYMBOLS } from "./sheet-row";

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

// The sample of board SH-Large, in part.
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
}

/**
 * The sheet with a text field in place of the native search bar of the screen, which
 * jest does not draw (the device checks of app/README.md cover the native bar).
 */
function SearchHost(props: Omit<ConversationsSheetProps, "query">) {
  const [query, setQuery] = useState("");
  return (
    <>
      <TextInput accessibilityLabel="Search conversations" value={query} onChangeText={setQuery} />
      <ConversationsSheet {...props} query={query} />
    </>
  );
}

async function renderSheet(options: Options = {}): Promise<FakeCaller> {
  const caller = options.caller ?? sampleCaller();
  await render(
    <SearchHost
      caller={caller}
      machines={options.machines ?? MACHINES}
      now={NOW}
      onOpen={options.onOpen ?? (() => undefined)}
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
    .map((row) => String(row.props.accessibilityLabel).split(". ")[0] ?? "");
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
  it("leads with Needs you and Running of every machine, then the days", async () => {
    const caller = await renderSheet();
    expect(caller.callsOf("conversations.search").map((call) => call.machine)).toEqual(["d", "l"]);
    expect(screen.getAllByRole("header").map((header) => String(header.props.children))).toEqual([
      "Needs you",
      "Running",
      "Today",
      "Yesterday",
      "Older",
    ]);
    expect(rowTitles("sheet-sections")).toEqual([
      "Fix steer race in turn loop",
      "Prune old btrfs snapshots",
      "Migrate session store to SQLite",
      "Why is the fan loud",
      "Set up restic backups",
      "Clean up old docker images",
    ]);
  });

  it("draws a row with its status word, machine, folder and time", async () => {
    await renderSheet();
    const waiting = screen.getByLabelText(
      "Fix steer race in turn loop. Needs approval. desktop, efr. 20 minutes ago.",
    );
    expect(within(waiting).getByText("needs approval").props.className).toBe("text-accent-primary");
    expect(within(waiting).getByText("efr").props.className).toBe("text-mono-code");
    expect(within(waiting).getByText("20m")).toBeOnTheScreen();
    expect(within(waiting).getByText("Fix steer race in turn loop").props.className).toBe(
      "text-row-strong text-text-bright",
    );
    const fan = screen.getByLabelText("Why is the fan loud. desktop, ~. 2 minutes ago.");
    expect(within(fan).getByText("Why is the fan loud").props.className).toBe(
      "text-row text-text-primary",
    );
    expect(within(fan).getByText("2m")).toBeOnTheScreen();
    expect(
      screen.getByLabelText("Set up restic backups. laptop, ~. Yesterday at 20:00."),
    ).toBeOnTheScreen();
    expect(screen.getByText("20:00")).toBeOnTheScreen();
    expect(screen.getByText("Oct 19")).toBeOnTheScreen();
  });

  it("gives each row the icon of its status, yolk for waiting and running", async () => {
    await renderSheet();
    const hidden = { includeHiddenElements: true } as const;
    expect(screen.getAllByTestId("row-icon-wait", hidden)).toHaveLength(2);
    expect(screen.getAllByTestId("row-icon-running", hidden)).toHaveLength(1);
    expect(screen.getAllByTestId("row-icon-none", hidden)).toHaveLength(3);
    const migrate = screen.getByLabelText(
      "Migrate session store to SQLite. Running. desktop, efr. 3 minutes ago.",
    );
    expect(within(migrate).getByText("3m")).toBeOnTheScreen();
    expect(ROW_SYMBOLS).toEqual({
      wait: { name: "exclamationmark.circle.fill", size: 19, tone: "accent_primary" },
      none: { name: "bubble.left", size: 17, tone: "border_subtle" },
    });
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
    // The phone keeps no rows of the laptop, so the banner tells no age.
    expect(within(screen.getByTestId("sheet-sections")).getByText("laptop offline")).toBeTruthy();
    expect(screen.queryByText(/did not answer/)).toBeNull();
  });

  it("heads the list with the banner of an offline machine and the age of its rows", async () => {
    jest.setSystemTime(NOW);
    // The list of the last visit, kept on the disk two hours ago, so stale.
    client.setQueryData(conversationsKeys.search("l", ""), result(RECENT["l"] ?? []), {
      updatedAt: NOW - 120 * MINUTE,
    });
    const caller = new FakeCaller({
      "conversations.search": (params: ConversationsSearch, machine: string) => {
        if (machine === "l") {
          throw new ConnectionError("offline", "not connected");
        }
        return result(params.query === "" ? (RECENT[machine] ?? []) : []);
      },
    });
    await renderSheet({ caller });
    const banner = within(screen.getByTestId("sheet-sections")).getByTestId("sheet-offline");
    expect(banner.props.accessibilityLabel).toBe("laptop offline, rows from 2 hours ago");
    expect(within(banner).getByText("laptop offline · rows from 2h ago")).toBeOnTheScreen();
  });

  it("shows no banner while every machine answers", async () => {
    await renderSheet();
    expect(screen.queryByTestId("sheet-offline")).toBeNull();
  });

  it("draws the words of the row states of 0.0.12", async () => {
    const states: readonly Conversation[] = [
      { id: "q", title: "Pick a test database", at: NOW - MINUTE, status: "running" },
      { id: "x", title: "Expo SDK 58 upgrade", at: NOW - 60 * MINUTE },
    ];
    const caller = new FakeCaller({
      "conversations.search": (params: ConversationsSearch, machine: string) => {
        const found = result(machine === "d" && params.query === "" ? states : []);
        for (const row of found.rows) {
          row.conversation.summary.attention =
            row.conversation.summary.id === "q" ? "answer" : "failed";
        }
        return found;
      },
    });
    await renderSheet({ caller });
    // The phone answers a question of the model (S-Ask).
    const question = screen.getByLabelText(
      "Pick a test database. Needs an answer. desktop. 1 minute ago.",
    );
    expect(within(question).getByText("needs an answer").props.className).toBe(
      "text-accent-primary",
    );
    const failed = screen.getByLabelText("Expo SDK 58 upgrade. Failed. desktop. 1 hour ago.");
    expect(within(failed).getByText("✗ failed").props.className).toBe("text-state-error");
    expect(screen.getAllByRole("header").map((header) => String(header.props.children))).toEqual([
      "Needs you",
      "Today",
    ]);
  });

  it("shows the kept rows of a machine that did not answer as offline", async () => {
    // The list of the last visit, kept on the disk, and old enough to ask again.
    client.setQueryData(conversationsKeys.search("l", ""), result(RECENT["l"] ?? []), {
      updatedAt: 0,
    });
    const caller = new FakeCaller({
      "conversations.search": (params: ConversationsSearch, machine: string) => {
        if (machine === "l") {
          throw new ConnectionError("offline", "not connected");
        }
        return result(params.query === "" ? (RECENT[machine] ?? []) : []);
      },
    });
    await renderSheet({ caller });
    expect(screen.getByTestId("sheet-offline")).toBeOnTheScreen();
    // The approval in the kept list can be old: the row shows no status, by its day.
    const btrfs = screen.getByLabelText(
      "Prune old btrfs snapshots. laptop, offline, ~. 30 minutes ago.",
    );
    expect(within(btrfs).getByText("Prune old btrfs snapshots").props.className).toBe(
      "text-row text-text-muted",
    );
    expect(within(btrfs).queryByText("needs approval")).toBeNull();
    expect(screen.getAllByTestId("row-icon-wait", { includeHiddenElements: true })).toHaveLength(1);
    expect(
      screen.getByLabelText("Set up restic backups. laptop, offline, ~. Yesterday at 20:00."),
    ).toBeOnTheScreen();
  });
});

describe("the search of the sheet", () => {
  it("filters the loaded rows at once by machine, before efrd answers", async () => {
    const caller = await renderSheet();
    await type("laptop");
    expect(screen.getByRole("header", { name: "2 results" })).toBeOnTheScreen();
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
    // The word of the search is text_bright semibold in the title; a deep hit has none.
    const fan = screen.getByLabelText("Why is the fan loud. desktop, ~. 2 minutes ago.");
    expect(within(fan).getByText("fan").props.className).toBe("text-row-strong text-text-bright");
    expect(within(screen.getByTestId("sheet-results")).queryAllByText("curve")).toEqual([]);
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
    expect(screen.queryByText(/results?$/)).toBeNull();
  });

  it("shows the sections again when the field is cleared", async () => {
    await renderSheet();
    await type("laptop");
    expect(screen.queryByTestId("sheet-sections")).toBeNull();
    await type("");
    expect(screen.getByRole("header", { name: "Needs you" })).toBeOnTheScreen();
    expect(screen.queryByTestId("sheet-results")).toBeNull();
  });
});

describe("a row of the sheet", () => {
  it("opens its conversation, with its machine", async () => {
    const onOpen = jest.fn<(ref: ConversationAddress) => void>();
    await renderSheet({ onOpen });
    await fireEvent.press(
      screen.getByLabelText("Set up restic backups. laptop, ~. Yesterday at 20:00."),
    );
    expect(onOpen).toHaveBeenCalledWith({ machine: "l", id: "restic" });
  });

  it("opens its menu on a long press: Open, and Copy ID", async () => {
    const onOpen = jest.fn<(ref: ConversationAddress) => void>();
    await renderSheet({ onOpen });
    const native = screen
      .getAllByTestId("menu-view")
      .find((element) => element.props.title === "Why is the fan loud");
    if (native === undefined) {
      throw new Error("the row of the fan has no menu");
    }
    expect(native.props.shouldOpenOnLongPress).toBe(true);
    expect(native.props.actions.map((action: { title: string }) => action.title)).toEqual([
      "Open",
      "Copy ID",
    ]);
    await fireEvent(native, "pressAction", { nativeEvent: { event: "copy-id" } });
    expect(jest.mocked(copyText).mock.calls).toEqual([["fan"]]);
    expect(haptic).toHaveBeenCalledWith("light");
    await fireEvent(native, "pressAction", { nativeEvent: { event: "open" } });
    expect(onOpen).toHaveBeenCalledWith({ machine: "d", id: "fan" });
  });

  it("gives VoiceOver the actions of the menu, and says that it has them", async () => {
    await renderSheet();
    const row = screen.getByLabelText("Why is the fan loud. desktop, ~. 2 minutes ago.");
    expect(row.props.accessibilityHint).toBe("Swipe up or down for actions.");
    expect(row.props.accessibilityActions).toEqual([
      { name: "open", label: "Open" },
      { name: "copy-id", label: "Copy ID" },
    ]);
    await fireEvent(row, "accessibilityAction", { nativeEvent: { actionName: "copy-id" } });
    expect(copyText).toHaveBeenCalledWith("fan");
  });
});
