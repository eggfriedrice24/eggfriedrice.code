// The states of the composer on the design canvas (S-Composer, Main, C-Offline and
// C-Reconnecting) and its actions: send through the outbox, steer, queue and stop.

import { describe, expect, it, jest } from "@jest/globals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createAtom, createStore, type Atom, type Readable } from "@tanstack/store";
import { act, fireEvent, render, screen } from "@testing-library/react-native";
import { processColor, StyleSheet } from "react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import type { ConnectionView } from "@/features/machines/model/connection-view";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import { createPreferences, type PreferencesStore } from "@/features/settings/model/preferences";
import { FakeCaller } from "@/lib/query/testing";
import type { KeyValueStorage } from "@/lib/store/key-value";
import type { StreamState } from "@/lib/stream/reducer";
import { eventItem, promptQueued, run, turnCompleted, turnStarted } from "@/lib/stream/testing";
import { RADII, SPACING } from "@/theme/metrics";
import { roles } from "@/theme/roles";

import { composerDraftKey, type ComposerTarget } from "../../model/composer-state";
import { createDrafts, type Drafts } from "../../model/drafts";
import type { Outbox, OutboxEntry } from "../../model/outbox";
import { CONVERSATION_A, DESKTOP, idMaker, testOutbox } from "../../model/outbox-testing";
import type { ComposerDeps } from "../../use-composer-actions";
import { DockedComposer, type DockedComposerProps } from "./composer";
import { bottomEdge, KEYBOARD_GAP, SCRIM_CLASS } from "./composer-root";
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

const HIDDEN = { includeHiddenElements: true } as const;
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
  it("is blank on the root: the pill and the status line of a new conversation, Send off", async () => {
    await renderComposer({ target: ROOT });
    expect(screen.getByTestId("composer-pill")).toContainElement(
      screen.getByTestId("composer-status-line"),
    );
    expect(screen.getByText("~")).toBeOnTheScreen();
    expect(screen.getByText("cautious")).toBeOnTheScreen();
    expect(screen.queryByText(/^ctx/)).toBeNull();
    expect(screen.getByTestId("material-glass", HIDDEN)).toBeOnTheScreen();
    expect(input().props.placeholder).toBe("Message efr");
    expect(sendButton()).toBeDisabled();
  });

  it("is idle after a turn: the status line inside the glass pill, Send off", async () => {
    await renderComposer({ stream: endedStream() });
    expect(screen.getByTestId("composer-status-line")).toBeOnTheScreen();
    expect(screen.getByText("~/code/efr")).toBeOnTheScreen();
    expect(screen.getByText("cautious")).toBeOnTheScreen();
    expect(screen.getByText("ctx 42%")).toBeOnTheScreen();
    expect(screen.getByTestId("composer-pill")).toContainElement(
      screen.getByTestId("composer-status-line"),
    );
    expect(sendButton()).toBeDisabled();
  });

  it("is a glass pill over a scrim, with no blur dock and no hairline", async () => {
    await renderComposer({ stream: endedStream() });
    const pill = screen.getByTestId("composer-pill");
    expect(pill.props.className).toBe("overflow-hidden rounded-composer");
    const glass = screen.getByTestId("material-glass", HIDDEN);
    expect(StyleSheet.flatten(glass.props.style)).toMatchObject({ borderRadius: RADII.composer });
    expect(pill).toContainElement(glass);
    expect(screen.getByTestId("composer-scrim", HIDDEN).props.className).toBe(SCRIM_CLASS);
    expect(SCRIM_CLASS).toContain("from-surface-1/0 via-surface-1/60 to-surface-1/90");
    expect(screen.queryByTestId("material-chrome", HIDDEN)).toBeNull();
    expect(screen.queryByTestId("hairline", HIDDEN)).toBeNull();
    expect(screen.getByTestId("composer-status-line").props.className).toContain("h-30");
    expect(screen.getByTestId("composer-status-line").props.className).toContain("px-16");
  });

  it("gives the field no border of its own, and the action a 32 pt circle", async () => {
    await renderComposer();
    expect(input().props.className).not.toContain("border");
    expect(input().props.className).not.toContain("rounded");
    expect(sendButton().props.className).toContain("w-action h-action");
    expect(sendButton().props.hitSlop).toBe((SPACING.hit - SPACING.action) / 2);
    expect(SPACING.action).toBe(32);
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
    expect(
      menu.props.actions.map((action: { id: string; image: string; state: string }) => [
        action.id,
        action.image,
        action.state,
      ]),
    ).toEqual([
      ["steer", "arrow.turn.left.up", "on"],
      ["queue", "list.number", "off"],
    ]);
    // Every glyph has a colour, so Fabric does not draw it clear.
    expect(menu.props.actions.map((action: { imageColor?: number }) => action.imageColor)).toEqual([
      processColor(roles.text_bright),
      processColor(roles.text_bright),
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

  it("docks to the keyboard with 6 pt over it and the safe area and 6 pt under it", async () => {
    await renderComposer();
    const dock = screen.getByTestId("composer");
    expect(dock.props.offset).toEqual({ closed: 0, opened: 34 });
    const frame = StyleSheet.flatten(dock.props.children.props.style);
    expect(frame).toMatchObject({ paddingBottom: 34 + KEYBOARD_GAP });
    expect(KEYBOARD_GAP).toBe(6);
  });

  it("keeps a bottom edge of 12 pt on a screen without a home indicator", () => {
    expect(bottomEdge(0)).toBe(12);
    expect(bottomEdge(34)).toBe(34);
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
