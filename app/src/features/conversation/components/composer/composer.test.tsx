// The states of the composer on the design canvas (S-Composer, Main, C-Offline and
// C-Reconnecting) and its actions: send through the outbox, steer, queue and stop.

import { describe, expect, it, jest } from "@jest/globals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createAtom, createStore, type Atom, type Readable } from "@tanstack/store";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import type { ConnectionView } from "@/features/machines/model/connection-view";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import { createPreferences, type PreferencesStore } from "@/features/settings/model/preferences";
import { FakeCaller } from "@/lib/query/testing";
import type { KeyValueStorage } from "@/lib/store/key-value";
import type { StreamState } from "@/lib/stream/reducer";
import { eventItem, promptQueued, run, turnCompleted, turnStarted } from "@/lib/stream/testing";

import { composerDraftKey, type ComposerTarget } from "../../model/composer-state";
import { createDrafts, type Drafts } from "../../model/drafts";
import type { Outbox, OutboxEntry } from "../../model/outbox";
import { CONVERSATION_A, DESKTOP, idMaker, testOutbox } from "../../model/outbox-testing";
import type { ComposerDeps } from "../../use-composer-actions";
import { DockedComposer, type DockedComposerProps } from "./composer";
import { sendModeEntries } from "./composer-trailing";

jest.mock("react-native-keyboard-controller", () =>
  jest.requireActual("react-native-keyboard-controller/jest"),
);

// @tanstack/pacer ships only ES modules, which jest-expo does not transform. The drafts
// test in Vitest covers the real debounce; here a save runs at once.
jest.mock("@tanstack/pacer", () => ({
  Debouncer: class {
    private readonly save: () => void;
    constructor(save: () => void) {
      this.save = save;
    }
    maybeExecute(): void {
      this.save();
    }
    flush(): void {
      // Each save already ran.
    }
    cancel(): void {
      // No save waits.
    }
  },
}));

/** A key-value store in memory. */
class MemoryKeyValue implements KeyValueStorage {
  readonly items = new Map<string, string>();
  getItemSync(key: string): string | null {
    return this.items.get(key) ?? null;
  }
  getItem(key: string): Promise<string | null> {
    return Promise.resolve(this.items.get(key) ?? null);
  }
  setItem(key: string, value: string): Promise<void> {
    this.items.set(key, value);
    return Promise.resolve();
  }
  removeItem(key: string): Promise<void> {
    this.items.delete(key);
    return Promise.resolve();
  }
}

const ENDPOINT = "100.64.0.1:7878";
const ONLINE: ConnectionView = { kind: "online", endpoint: ENDPOINT };
const METRICS = {
  frame: { x: 0, y: 0, width: 402, height: 874 },
  insets: { top: 62, left: 0, right: 0, bottom: 34 },
};

const CONVERSATION: ComposerTarget = { machine: DESKTOP, id: CONVERSATION_A, isBlankRoot: false };
const ROOT: ComposerTarget = { machine: DESKTOP, id: "root-local", isBlankRoot: true };

/** The stream of a conversation whose last turn ran in ~/code/efr, cautious, at 42%. */
function endedStream(): Readable<StreamState> {
  const started = {
    ...turnStarted("t1"),
    cwd: "/home/egg/code/efr",
    settings: { mode: "cautious", model: "m" },
  } as const;
  const ended = {
    ...turnCompleted("t1"),
    context: { tokens: 42_000, limit: 100_000, window: 100_000 },
  };
  return createStore(
    run([eventItem(1, promptQueued("t1", "hi")), eventItem(2, started), eventItem(3, ended)]),
  );
}

interface Harness {
  readonly caller: FakeCaller;
  readonly outbox: Outbox;
  readonly drafts: Drafts;
  readonly preferences: PreferencesStore;
  readonly connection: Atom<MachineConnection>;
  readonly sent: OutboxEntry[];
}

interface Setup {
  readonly target?: ComposerTarget;
  readonly view?: ConnectionView;
  readonly runningTurnId?: string;
  readonly text?: string;
  readonly stream?: Readable<StreamState>;
  readonly tailnetDown?: boolean;
}

async function renderComposer(setup: Setup = {}): Promise<Harness> {
  const target = setup.target ?? CONVERSATION;
  const caller = new FakeCaller();
  const { outbox } = testOutbox();
  const storage = new MemoryKeyValue();
  const drafts = createDrafts({ storage });
  const preferences = createPreferences({ storage });
  const connection = createAtom<MachineConnection>({
    client: { state: "off" },
    view: setup.view ?? ONLINE,
    seenAt: undefined,
  });
  if (setup.text !== undefined) {
    drafts.set(composerDraftKey(target), setup.text);
  }
  const deps: ComposerDeps = {
    caller,
    outbox,
    drafts,
    preferences,
    newCommandId: idMaker("c0000000"),
  };
  const sent: OutboxEntry[] = [];
  // No garbage collection timer, so jest ends when the tests do.
  const client = new QueryClient({
    defaultOptions: {
      mutations: { retry: false, gcTime: Infinity },
      queries: { gcTime: Infinity },
    },
  });
  const props: { -readonly [K in keyof DockedComposerProps]: DockedComposerProps[K] } = {
    deps,
    target,
    connection,
    machineName: "laptop",
    onSend: (entry: OutboxEntry) => {
      sent.push(entry);
    },
  };
  if (setup.runningTurnId !== undefined) {
    props.runningTurnId = setup.runningTurnId;
  }
  if (setup.stream !== undefined) {
    props.stream = setup.stream;
  }
  if (setup.tailnetDown !== undefined) {
    props.tailnetDown = setup.tailnetDown;
  }
  await render(
    <SafeAreaProvider initialMetrics={METRICS}>
      <QueryClientProvider client={client}>
        <DockedComposer {...props} />
      </QueryClientProvider>
    </SafeAreaProvider>,
  );
  return { caller, outbox, drafts, preferences, connection, sent };
}

/** Lets TanStack Query tell the observers of a mutation (it notifies on a timer of 0 ms). */
async function settleQuery(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  });
}

function input() {
  return screen.getByLabelText("Message");
}

function sendButton() {
  return screen.getByRole("button", { name: "Send" });
}

describe("the states of the composer", () => {
  it("is blank on the root: no status line, no material, no hairline, Send off", async () => {
    await renderComposer({ target: ROOT });
    expect(screen.queryByTestId("composer-status-line")).toBeNull();
    expect(screen.queryByTestId("material-chrome")).toBeNull();
    expect(screen.queryByTestId("hairline", { includeHiddenElements: true })).toBeNull();
    expect(input().props.placeholder).toBe("Message efr");
    expect(sendButton()).toBeDisabled();
  });

  it("is idle after a turn: the status line, the material and the hairline, Send off", async () => {
    await renderComposer({ stream: endedStream() });
    expect(screen.getByTestId("composer-status-line")).toBeOnTheScreen();
    expect(screen.getByText("~/code/efr")).toBeOnTheScreen();
    expect(screen.getByText("cautious")).toBeOnTheScreen();
    expect(screen.getByText("ctx 42%")).toBeOnTheScreen();
    expect(screen.getByTestId("material-chrome")).toBeOnTheScreen();
    expect(screen.getByTestId("hairline", { includeHiddenElements: true })).toBeOnTheScreen();
    expect(sendButton()).toBeDisabled();
  });

  it("enables Send while typing, and the field grows to six lines", async () => {
    await renderComposer();
    await fireEvent.changeText(input(), "Now write the migration notes");
    expect(sendButton()).toBeEnabled();
    expect(sendButton().props.className).toContain("bg-accent-primary");
    expect(input().props.value).toBe("Now write the migration notes");
    expect(input().props.className).toContain("min-h-field max-h-field-max");
    expect(input().props.multiline).toBe(true);
  });

  it("shows Stop only while a turn runs with an empty field", async () => {
    await renderComposer({ runningTurnId: "t1" });
    expect(screen.getByRole("button", { name: "Stop the turn" })).toBeOnTheScreen();
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    expect(screen.queryByTestId("composer-capsule")).toBeNull();
    expect(input().props.placeholder).toBe("Steer or queue");
  });

  it("shows Stop and the Steer capsule while a turn runs with text", async () => {
    await renderComposer({ runningTurnId: "t1", text: "Use TurnState, not a new enum" });
    expect(screen.getByRole("button", { name: "Stop the turn" })).toBeOnTheScreen();
    const capsule = screen.getByRole("button", { name: "Steer, more options" });
    expect(capsule.props.className).toContain("bg-accent-primary");
    expect(screen.getByText("Steer")).toBeOnTheScreen();
  });

  it("opens the Steer/Queue menu on a long press, with the check on the mode", async () => {
    await renderComposer({ runningTurnId: "t1", text: "Use TurnState" });
    const menu = screen.getByTestId("composer-send-mode");
    expect(menu.props.shouldOpenOnLongPress).toBe(true);
    expect(menu.props.actions).toEqual([
      { id: "steer", title: "Steer now", subtitle: "Joins the running turn", state: "on" },
      { id: "queue", title: "Queue", subtitle: "Sends when this turn ends", state: "off" },
    ]);
    await fireEvent(menu, "openMenu");
    expect(screen.getByRole("button", { name: "Steer, more options" })).toBeExpanded();
    await fireEvent(menu, "closeMenu");
    expect(screen.getByRole("button", { name: "Steer, more options" })).toBeCollapsed();
  });

  it("shows the Queue capsule when queue is the last choice", async () => {
    const harness = await renderComposer({ runningTurnId: "t1", text: "Also add a test" });
    await act(async () => {
      harness.preferences.setSendMode("queue");
    });
    const capsule = screen.getByRole("button", { name: "Queue, more options" });
    expect(capsule.props.className).toContain("border border-accent-primary");
    expect(sendModeEntries("queue")[1]).toMatchObject({ id: "queue", state: "on" });
  });

  it("is sending while the prompt waits for efrd: dimmed text and the braille cell", async () => {
    const harness = await renderComposer({ text: "Now write the migration notes" });
    await fireEvent.press(sendButton());
    expect(screen.getByRole("button", { name: "Sending" })).toBeBusy();
    expect(screen.getByTestId("braille-cell", { includeHiddenElements: true })).toBeOnTheScreen();
    expect(input().props.value).toBe("Now write the migration notes");
    expect(input().props.className).toContain("text-text-subtle");
    // The native input stays editable, so the keyboard stays up, but takes no text.
    expect(input().props.editable).toBe(true);
    await fireEvent.changeText(input(), "more");
    expect(harness.drafts.text(composerDraftKey(CONVERSATION))).toBe("");
    // efrd took it: the field is empty again.
    const [entry] = harness.outbox.store.get().entries;
    if (entry === undefined) {
      throw new Error("the entry of the send");
    }
    await act(async () => {
      harness.outbox.begin(entry.commandId);
      harness.outbox.succeed(entry.commandId, {
        conversation_id: CONVERSATION_A,
        turn_id: "t2",
        seq: 9,
        queued: false,
      });
    });
    expect(input().props.value).toBe("");
    expect(sendButton()).toBeDisabled();
  });

  it("is disconnected when the machine does not answer: input off at 60%", async () => {
    const harness = await renderComposer({
      stream: endedStream(),
      view: { kind: "unreachable", endpoint: ENDPOINT },
    });
    expect(input().props.editable).toBe(false);
    expect(input().props.className).toContain("opacity-60");
    expect(input().props.placeholder).toBe("laptop is offline");
    expect(sendButton()).toBeDisabled();
    // The status line keeps the cwd, the mode and the context.
    expect(screen.getByText("~/code/efr")).toBeOnTheScreen();
    // A new try of the client does not turn the input on.
    await act(async () => {
      harness.connection.set({
        client: { state: "connecting" },
        view: { kind: "connecting", endpoint: ENDPOINT },
        seenAt: undefined,
      });
    });
    expect(input().props.editable).toBe(false);
    await act(async () => {
      harness.connection.set({ client: { state: "off" }, view: ONLINE, seenAt: undefined });
    });
    expect(input().props.editable).toBe(true);
  });

  it.each([
    [{ kind: "unreachable", endpoint: ENDPOINT } as const, true, "Tailscale is not connected"],
    [{ kind: "starting", endpoint: ENDPOINT } as const, false, "efrd is starting"],
    [{ kind: "repair_needed", reason: "node_changed" } as const, false, "re-pair needed"],
    [{ kind: "update_needed" } as const, false, "update needed"],
  ])("says why it is disconnected: %j", async (view, tailnetDown, words) => {
    await renderComposer({ view, tailnetDown });
    expect(input().props.placeholder).toBe(words);
    expect(input().props.editable).toBe(false);
  });

  it("is reconnecting while the connection opens: editable, Send off, a grey loader", async () => {
    await renderComposer({
      stream: endedStream(),
      view: { kind: "connecting", endpoint: ENDPOINT },
      text: "keep typing",
    });
    expect(input().props.editable).toBe(true);
    expect(sendButton()).toBeDisabled();
    expect(screen.getByText("reconnecting…")).toBeOnTheScreen();
    expect(screen.getByRole("status")).toBeOnTheScreen();
  });

  it("docks to the keyboard with 8 pt over it and the safe area under it", async () => {
    await renderComposer();
    const dock = screen.getByTestId("composer");
    expect(dock.props.offset).toEqual({ closed: 0, opened: 26 });
  });
});

describe("the actions of the composer", () => {
  it("sends through the outbox and clears the draft at once", async () => {
    const harness = await renderComposer({ text: "Now write the migration notes" });
    await fireEvent.press(sendButton());
    const entries = harness.outbox.store.get().entries;
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      machine: DESKTOP,
      target: { kind: "conversation", conversationId: CONVERSATION_A },
      text: "Now write the migration notes",
      state: "pending",
    });
    expect(harness.drafts.text(composerDraftKey(CONVERSATION))).toBe("");
    expect(harness.sent.map((entry) => entry.text)).toEqual(["Now write the migration notes"]);
  });

  it("makes a new conversation from the blank root", async () => {
    const harness = await renderComposer({ target: ROOT, text: "hello" });
    await fireEvent.press(sendButton());
    expect(harness.outbox.store.get().entries[0]?.target).toEqual({
      kind: "new",
      localId: "root-local",
    });
  });

  it("steers the running turn, late steers queue, and the draft goes once efrd took it", async () => {
    const harness = await renderComposer({ runningTurnId: "t1", text: "Use TurnState" });
    let answer: (() => void) | undefined;
    harness.caller.answer({
      "turn.steer": () =>
        new Promise((resolve) => {
          answer = () => resolve({ seq: 12, turn_id: "t1" });
        }),
    });
    await fireEvent.press(screen.getByRole("button", { name: "Steer, more options" }));
    await settleQuery();
    expect(harness.caller.callsOf("turn.steer")[0]?.params).toEqual({
      command_id: "c0000000-0000-4000-8000-000000000001",
      conversation_id: CONVERSATION_A,
      text: "Use TurnState",
      if_late: { kind: "queue" },
      turn_id: "t1",
    });
    expect(screen.getByRole("button", { name: "Sending" })).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Stop the turn" })).toBeOnTheScreen();
    expect(input().props.value).toBe("Use TurnState");
    await act(async () => {
      answer?.();
    });
    await settleQuery();
    expect(harness.drafts.text(composerDraftKey(CONVERSATION))).toBe("");
    expect(harness.outbox.store.get().entries).toHaveLength(0);
  });

  it("keeps the text of a steer that efrd refused", async () => {
    const harness = await renderComposer({ runningTurnId: "t1", text: "Use TurnState" });
    harness.caller.answer({
      "turn.steer": () => Promise.reject(new Error("conflict")),
    });
    await fireEvent.press(screen.getByRole("button", { name: "Steer, more options" }));
    await settleQuery();
    expect(harness.drafts.text(composerDraftKey(CONVERSATION))).toBe("Use TurnState");
    expect(screen.getByRole("button", { name: "Steer, more options" })).toBeOnTheScreen();
  });

  it("queues through the outbox in queue mode", async () => {
    const harness = await renderComposer({ runningTurnId: "t1", text: "Also add a test" });
    await act(async () => {
      harness.preferences.setSendMode("queue");
    });
    await fireEvent.press(screen.getByRole("button", { name: "Queue, more options" }));
    expect(harness.caller.callsOf("turn.steer")).toHaveLength(0);
    expect(harness.outbox.store.get().entries[0]?.text).toBe("Also add a test");
  });

  it("picks a mode in the menu: it sticks, and the text goes in that mode", async () => {
    const harness = await renderComposer({ runningTurnId: "t1", text: "Also add a test" });
    await fireEvent(screen.getByTestId("composer-send-mode"), "pressAction", {
      nativeEvent: { event: "queue" },
    });
    expect(harness.preferences.store.get().sendMode).toBe("queue");
    expect(harness.outbox.store.get().entries[0]?.text).toBe("Also add a test");
    expect(harness.caller.callsOf("turn.steer")).toHaveLength(0);
  });

  it("stops the running turn with no scope, once per wait", async () => {
    const harness = await renderComposer({ runningTurnId: "t1" });
    harness.caller.answer({
      "turn.interrupt": () => new Promise(() => undefined),
    });
    const stop = screen.getByRole("button", { name: "Stop the turn" });
    await fireEvent.press(stop);
    await settleQuery();
    await fireEvent.press(stop);
    expect(harness.caller.callsOf("turn.interrupt").map((call) => call.params)).toEqual([
      {
        command_id: "c0000000-0000-4000-8000-000000000001",
        conversation_id: CONVERSATION_A,
        turn_id: "t1",
      },
    ]);
  });

  it("writes the draft of each key press to the drafts store", async () => {
    const harness = await renderComposer();
    await fireEvent.changeText(input(), "a");
    await fireEvent.changeText(input(), "ab");
    expect(harness.drafts.text(composerDraftKey(CONVERSATION))).toBe("ab");
  });
});
