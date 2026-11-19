// The states of the composer on the design canvas (S-Composer, Main, C-Offline and
// C-Reconnecting) and its actions: send through the outbox, steer, queue and stop.

import { describe, expect, it, jest } from "@jest/globals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createAtom, createStore, type Atom, type Readable } from "@tanstack/store";
import { act, fireEvent, render, screen, within } from "@testing-library/react-native";
import { Linking, processColor, StyleSheet, TextInput } from "react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import type { ConnectionView } from "@/features/machines/model/connection-view";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import { createPreferences, type PreferencesStore } from "@/features/settings/model/preferences";
import { FakeClock, remoteHello } from "@/lib/net/testing";
import { DEMO_ATTACHMENT_LIMITS as LIMITS } from "@/features/demo/model/demo-uploads";
import type { ModelsListResult } from "@/lib/protocol/generated";
import { FakeCaller } from "@/lib/query/testing";
import type { KeyValueStorage } from "@/lib/store/key-value";
import type { StreamState } from "@/lib/stream/reducer";
import { eventItem, promptQueued, run, turnCompleted, turnStarted } from "@/lib/stream/testing";
import { RADII, SPACING, TILE } from "@/theme/metrics";
import { roles } from "@/theme/roles";

import { testAttach, type TestAttach } from "../../model/attachments-testing";
import { MICROPHONE_DENIED, VOICE_LINES, createVoice } from "../../model/voice";
import { FakeSpeech } from "../../model/voice-testing";
import { composerDraftKey, type ComposerTarget } from "../../model/composer-state";
import { createDrafts, type Drafts } from "../../model/drafts";
import { createModelChoices, type ModelChoice, type ModelChoices } from "../../model/model-choices";
import type { Outbox, OutboxEntry } from "../../model/outbox";
import { CONVERSATION_A, DESKTOP, idMaker, testOutbox } from "../../model/outbox-testing";
import { DEFAULT_FACTS } from "@/lib/lock/owner-check";
import { initialLock, type LockState } from "@/features/navigation/model/app-lock";
import type { ComposerDeps } from "../../use-composer-actions";
import { DockedComposer, type DockedComposerProps } from "./composer";
import type { ComposerAnswer } from "./composer-context";
import { bottomEdge, KEYBOARD_GAP, SCRIM_CLASS } from "./composer-root";
import { sendModeEntries } from "./composer-trailing";

jest.mock("react-native-keyboard-controller", () =>
  jest.requireActual("react-native-keyboard-controller/jest"),
);

/** The wall clock of the tests, in ms since the epoch: the time of a recording stands still. */
const mockNow = Date.UTC(2026, 10, 18, 14, 0);

jest.mock("../use-now", () => ({ useNow: () => mockNow }));

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
  readonly modelChoices: ModelChoices;
  readonly connection: Atom<MachineConnection>;
  readonly sent: OutboxEntry[];
  /** How often the model button opened the sheet. */
  readonly opened: { count: number };
  /** efr-speech of the composer, when the setup asked for voice. */
  readonly speech: FakeSpeech | undefined;
}

interface Setup {
  readonly target?: ComposerTarget;
  readonly view?: ConnectionView;
  readonly runningTurnId?: string;
  readonly text?: string;
  readonly stream?: Readable<StreamState>;
  readonly tailnetDown?: boolean;
  /** False: no `onSend`, as on a conversation screen. */
  readonly onSend?: boolean;
  /** The model that the sheet chose for this composer. */
  readonly choice?: ModelChoice;
  /** The answer of `models.list`; with it the status line has the model button. */
  readonly models?: ModelsListResult;
  /** The Face ID unlock of the app is on (and open). */
  readonly lockOn?: boolean;
  /** True to leave the input without focus, so the composer rests as a one-line pill. */
  readonly rest?: boolean;
  /** The question of the model that Send answers (S-Ask 3). */
  readonly answering?: ComposerAnswer;
  /** The pickers and the files; with them efrd sends the capability `attachments`. */
  readonly attach?: (outbox: Outbox) => TestAttach;
  /** True for efr-speech on an iPhone with speech support for its language. */
  readonly voice?: boolean;
}

/** A Face ID unlock that is open, or off. */
function lockState(on: boolean): LockState {
  const off = initialLock({ on: false, notice: null });
  return on ? { ...off, on: true, open: true, shown: true, phase: "active" } : off;
}

async function renderComposer(setup: Setup = {}): Promise<Harness> {
  const target = setup.target ?? CONVERSATION;
  const models = setup.models;
  const caller = new FakeCaller(models === undefined ? {} : { "models.list": () => models });
  const { outbox } = testOutbox();
  const storage = new MemoryKeyValue();
  const drafts = createDrafts({ storage });
  const preferences = createPreferences({ storage });
  const modelChoices = createModelChoices({ storage });
  if (setup.choice !== undefined) {
    modelChoices.set(composerDraftKey(target), setup.choice);
  }
  const opened = { count: 0 };
  const attach = setup.attach?.(outbox);
  const connection = createAtom<MachineConnection>({
    client:
      attach === undefined
        ? { state: "off" }
        : {
            state: "ready",
            hello: remoteHello({ capabilities: { attachments: LIMITS } }),
            scopes: ["read", "operate"],
          },
    view: setup.view ?? ONLINE,
    seenAt: undefined,
  });
  if (setup.text !== undefined) {
    drafts.set(composerDraftKey(target), setup.text);
  }
  const speech = setup.voice === true ? new FakeSpeech() : undefined;
  const voice =
    speech === undefined
      ? null
      : createVoice({
          speech,
          drafts,
          clock: new FakeClock(),
          now: () => mockNow,
          haptic: () => undefined,
          onError: () => undefined,
        });
  // The voice controller asks the module for support once, at once.
  await act(async () => {
    await Promise.resolve();
  });
  const deps: ComposerDeps = {
    caller,
    outbox,
    drafts,
    preferences,
    modelChoices,
    newCommandId: idMaker("c0000000"),
    lock: {
      store: createStore(lockState(setup.lockOn === true)),
      facts: () => Promise.resolve(DEFAULT_FACTS),
    },
    attach: attach ?? null,
    voice,
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
  };
  if (setup.onSend !== false) {
    props.onSend = (entry: OutboxEntry) => {
      sent.push(entry);
    };
  }
  if (setup.runningTurnId !== undefined) {
    props.runningTurnId = setup.runningTurnId;
  }
  if (setup.stream !== undefined) {
    props.stream = setup.stream;
  }
  if (setup.tailnetDown !== undefined) {
    props.tailnetDown = setup.tailnetDown;
  }
  if (setup.answering !== undefined) {
    props.answering = setup.answering;
  }
  if (models !== undefined) {
    props.onOpenModels = () => {
      opened.count += 1;
    };
  }
  await render(
    <SafeAreaProvider initialMetrics={METRICS}>
      <QueryClientProvider client={client}>
        <DockedComposer {...props} />
      </QueryClientProvider>
    </SafeAreaProvider>,
  );
  // Most tests look at the full composer, which shows while the input has focus.
  if (setup.rest !== true) {
    await fireEvent(
      screen.getByLabelText(setup.answering === undefined ? "Message" : "Your answer"),
      "focus",
    );
  }
  return { caller, outbox, drafts, preferences, modelChoices, connection, sent, opened, speech };
}

/** Lets TanStack Query tell the observers of a mutation (it notifies on a timer of 0 ms). */
async function settleQuery(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  });
}

/** The `focus` of every TextInput: the jest mock puts its native methods on the prototype. */
function textInputFocus(): jest.Mock {
  const prototype: unknown = Reflect.get(TextInput, "prototype");
  const focus: unknown =
    typeof prototype === "object" && prototype !== null
      ? Reflect.get(prototype, "focus")
      : undefined;
  if (!jest.isMockFunction(focus)) {
    throw new Error("the jest mock of TextInput has no focus");
  }
  return focus;
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
    // On a narrow line the path gives way first, then ctx, then the model button
    // (AC-Limits 3, S-ComposerR7 6). Each part holds its own "·", so a part that went
    // takes its dot with it.
    const cwd = screen.getByTestId("composer-status-cwd");
    expect(cwd.props.className).toContain("shrink-[1000000]");
    expect(cwd.props.className).toContain("overflow-hidden");
    expect(cwd).toContainElement(screen.getByText("~/code/efr"));
    expect(within(cwd).getByText("·", HIDDEN)).toBeOnTheScreen();
    const context = screen.getByTestId("composer-status-context");
    expect(context.props.className).toContain("shrink-[100]");
    expect(context).toContainElement(screen.getByText("ctx 42%"));
    expect(screen.getByText("cautious")).toBeOnTheScreen();
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

  it.each([true, false])(
    "has the plus at the left and the voice button before Send (rest: %s)",
    async (rest) => {
      await renderComposer({ rest, attach: testAttach, voice: true });
      const attach = screen.getByRole("button", { name: "Attach" });
      const voice = screen.getByRole("button", { name: "Record a voice message" });
      expect(attach).toBeEnabled();
      expect(attach.props.accessibilityHint).toBe("Photo Library, Take Photo or Choose File.");
      expect(voice).toBeEnabled();
      for (const button of [attach, voice]) {
        expect(button.props.className).toContain("w-action h-action");
        expect(button.props.hitSlop).toBe((SPACING.hit - SPACING.action) / 2);
      }
      // The order of the row (attach, the field, voice, Send) is device check 25.
      expect(sendButton()).toBeOnTheScreen();
    },
  );

  it("has no plus and no microphone, and nothing in their place, in a build without them", async () => {
    await renderComposer();
    expect(screen.queryByTestId("composer-attach")).toBeNull();
    expect(screen.queryByTestId("composer-voice")).toBeNull();
    expect(sendButton()).toBeOnTheScreen();
  });

  it("enables Send while typing, and the field grows to six lines", async () => {
    await renderComposer();
    await fireEvent.changeText(input(), "Now write the migration notes");
    expect(sendButton()).toBeEnabled();
    expect(sendButton().props.className).toContain("bg-accent-primary");
    expect(input().props.value).toBe("Now write the migration notes");
    // Focused: at least 72 pt (about 3 lines), up to six lines.
    expect(input().props.className).toContain("min-h-field-focused");
    expect(input().props.className).toContain("max-h-field-max");
    expect(SPACING.fieldFocused).toBe(72);
    expect(input().props.scrollEnabled).toBe(true);
    expect(input().props.multiline).toBe(true);
  });

  it("rests as a one-line pill without focus, and grows to the full composer on focus", async () => {
    await renderComposer({
      stream: endedStream(),
      rest: true,
      text: "Line one\nLine two",
      attach: testAttach,
    });
    // At rest: no status line, a one-line field that does not scroll, the action stays.
    expect(screen.queryByTestId("composer-status-line")).toBeNull();
    expect(input().props.className).toContain("min-h-field max-h-field");
    expect(input().props.className).not.toContain("max-h-field-max");
    expect(input().props.className).not.toContain("min-h-field-focused");
    expect(input().props.scrollEnabled).toBe(false);
    // One row: the field takes the room between the buttons, with no row of actions.
    expect(input().props.className).toContain("flex-1 px-4");
    expect(screen.queryByTestId("composer-actions")).toBeNull();
    expect(sendButton()).toBeEnabled();
    await fireEvent(input(), "focus");
    expect(screen.getByTestId("composer-status-line")).toBeOnTheScreen();
    expect(screen.getByText("~/code/efr")).toBeOnTheScreen();
    expect(input().props.className).toContain("min-h-field-focused max-h-field-max");
    // The full width under the status line, with its 16 pt sides; the actions go under it.
    expect(input().props.className).toContain("px-16");
    expect(input().props.className).not.toContain("flex-1");
    expect(screen.getByTestId("composer-actions")).toContainElement(sendButton());
    expect(screen.getByTestId("composer-actions")).toContainElement(
      screen.getByTestId("composer-attach"),
    );
    expect(input().props.scrollEnabled).toBe(true);
    // A blur makes it a pill again; the draft stays.
    await fireEvent(input(), "blur");
    expect(screen.queryByTestId("composer-status-line")).toBeNull();
    expect(screen.queryByTestId("composer-actions")).toBeNull();
    expect(input().props.value).toBe("Line one\nLine two");
  });

  it("draws the placeholder smaller than the text", async () => {
    await renderComposer();
    expect(input().props.className).toContain("text-input-empty");
    await fireEvent.changeText(input(), "a");
    expect(input().props.className).toContain("text-input");
    expect(input().props.className).not.toContain("text-input-empty");
  });

  it("focuses the input on a tap on the resting pill", async () => {
    await renderComposer({ rest: true });
    const focus = textInputFocus();
    focus.mockClear();
    await fireEvent.press(screen.getByTestId("composer-pill"));
    expect(focus).toHaveBeenCalledTimes(1);
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
    // The input takes no focus, so the composer rests as a pill and the placeholder says
    // why.
    expect(screen.queryByTestId("composer-status-line")).toBeNull();
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

  it("focuses the input on a tap anywhere on the pill", async () => {
    await renderComposer({ stream: endedStream() });
    const focus = textInputFocus();
    focus.mockClear();
    await fireEvent.press(screen.getByTestId("composer-pill"));
    expect(focus).toHaveBeenCalledTimes(1);
  });

  it("takes no focus from a tap on the pill while disconnected", async () => {
    await renderComposer({
      stream: endedStream(),
      view: { kind: "unreachable", endpoint: ENDPOINT },
    });
    const focus = textInputFocus();
    focus.mockClear();
    await fireEvent.press(screen.getByTestId("composer-pill"));
    expect(focus).not.toHaveBeenCalled();
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

  it("sends the model, the account and the effort that the sheet chose", async () => {
    const choice = { model: "gpt-6.1-sol", provider: "openai-subscription.b", effort: "medium" };
    const harness = await renderComposer({ text: "Review the fix.", choice });
    await fireEvent.press(sendButton());
    expect(harness.outbox.store.get().entries[0]?.settings).toEqual(choice);
  });

  it("names the model, the account and the effort on the model button, which opens the sheet", async () => {
    const efforts = ["low", "medium", "high"];
    const models: ModelsListResult = {
      models: [
        { id: "gpt-6.1-sol", provider: "openai-subscription", source: "builtin", efforts },
        { id: "gpt-6.1-sol", provider: "openai-subscription.b", source: "builtin", efforts },
      ],
    };
    const choice = { model: "gpt-6.1-sol", provider: "openai-subscription.b", effort: "medium" };
    const harness = await renderComposer({ models, choice, stream: endedStream() });
    await settleQuery();
    const button = screen.getByTestId("composer-model-button");
    expect(button).toHaveTextContent("GPT-6.1 Sol · b · medium");
    expect(button).toHaveAccessibleName(
      "Model: GPT-6.1 Sol on b, medium effort, chosen for this conversation",
    );
    // A chosen model is text_primary, as MS-Switched draws it.
    expect(screen.getByText("GPT-6.1 Sol · b · medium").props.className).toContain(
      "text-text-primary",
    );
    await fireEvent.press(button);
    expect(harness.opened.count).toBe(1);
  });

  it("sends and queues with no onSend, as a conversation screen has it", async () => {
    const harness = await renderComposer({ onSend: false, text: "And the disk?" });
    await fireEvent.press(sendButton());
    expect(harness.outbox.store.get().entries.map((entry) => entry.text)).toEqual([
      "And the disk?",
    ]);
    expect(harness.drafts.text(composerDraftKey(CONVERSATION))).toBe("");
  });

  it("queues with no onSend while a turn runs", async () => {
    const harness = await renderComposer({ onSend: false, runningTurnId: "t1", text: "Then?" });
    await act(async () => {
      harness.preferences.setSendMode("queue");
    });
    await fireEvent.press(screen.getByRole("button", { name: "Queue, more options" }));
    expect(harness.outbox.store.get().entries.map((entry) => entry.text)).toEqual(["Then?"]);
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

/** Picks `mode` in the mode menu of the composer. */
async function pickMode(mode: "cautious" | "auto"): Promise<void> {
  await fireEvent(screen.getByTestId("composer-mode-menu"), "pressAction", {
    nativeEvent: { event: mode },
  });
}

describe("the mode and the Face ID unlock (S-ComposerR7 round 9, AU-Auto r9 4)", () => {
  it("is cautious by default, and the menu offers Cautious and Auto, never Full", async () => {
    await renderComposer({ text: "Push the fix.", lockOn: true });
    await settleQuery();
    const button = screen.getByTestId("composer-mode-button");
    expect(button).toHaveTextContent("cautious");
    expect(button).toHaveAccessibleName(
      "Mode: cautious. efr asks you before a call leaves the sandbox.",
    );
    const menu = screen.getByTestId("composer-mode-menu");
    expect(
      menu.props.actions.map((action: { id: string; subtitle: string; state: string }) => [
        action.id,
        action.subtitle,
        action.state,
      ]),
    ).toEqual([
      ["cautious", "efr asks you before a call leaves the sandbox.", "on"],
      ["auto", "A reviewer decides each call.", "off"],
    ]);
    await pickMode("auto");
    expect(screen.getByTestId("composer-mode-button")).toHaveTextContent("🤖auto");
  });

  it("greys Auto and offers to turn the unlock on while the unlock is off", async () => {
    await renderComposer({ text: "Push the fix." });
    await settleQuery();
    const actions = screen.getByTestId("composer-mode-menu").props.actions;
    expect(actions.find((action: { id: string }) => action.id === "auto")).toMatchObject({
      subtitle: "Needs Face ID unlock in Settings.",
      attributes: { disabled: true },
    });
    const section = actions.at(-1);
    expect(section.subactions[0]).toMatchObject({ title: "Turn on Face ID unlock" });
    await pickMode("auto");
    expect(screen.getByTestId("composer-mode-button")).toHaveTextContent("cautious");
  });

  it("sends in auto at once while the unlock is on: no check, and Send says Send", async () => {
    const harness = await renderComposer({ text: "Push the fix.", lockOn: true });
    await pickMode("auto");
    expect(sendButton()).toBeOnTheScreen();
    await fireEvent.press(sendButton());
    expect(harness.outbox.store.get().entries[0]).toMatchObject({
      text: "Push the fix.",
      auto: true,
    });
  });

  it("shows cautious and sends no auto for a stored auto while the unlock is off", async () => {
    const harness = await renderComposer({ text: "Push the fix." });
    await act(async () => {
      harness.modelChoices.setMode(composerDraftKey(CONVERSATION), "auto");
    });
    expect(screen.getByTestId("composer-mode-button")).toHaveTextContent("cautious");
    await fireEvent.press(sendButton());
    expect(harness.outbox.store.get().entries[0]?.auto).toBeUndefined();
  });

  it("steers at once into a running turn", async () => {
    const harness = await renderComposer({
      runningTurnId: "t1",
      text: "Use TurnState",
      lockOn: true,
    });
    harness.caller.answer({ "turn.steer": () => ({ seq: 12, turn_id: "t1" }) });
    await fireEvent.press(screen.getByRole("button", { name: "Steer, more options" }));
    await settleQuery();
    expect(harness.caller.callsOf("turn.steer")).toHaveLength(1);
  });
});

/** How an answer state of a test starts. */
interface AnswerSetup {
  /** The typed answer. */
  readonly text?: string;
  readonly sending?: boolean;
  readonly failed?: boolean;
}

/** An answer state that writes down what the composer sends and each leave. */
function answering(
  sent: string[],
  left: { count: number },
  setup: AnswerSetup = {},
): ComposerAnswer & { readonly draft: ReturnType<typeof createStore<string>> } {
  const draft = createStore(setup.text ?? "");
  return {
    question: "Which database should the tests use?",
    draft,
    setText: (text) => draft.setState(() => text),
    sending: setup.sending === true,
    failed: setup.failed === true,
    send: (text) => {
      sent.push(text);
    },
    leave: () => {
      left.count += 1;
    },
  };
}

describe("the answer state of the composer (S-Ask 3)", () => {
  it("shows the answer row in place of the status line, and the hint under the field", async () => {
    await renderComposer({ answering: answering([], { count: 0 }), runningTurnId: "t1" });
    expect(screen.getByTestId("composer-answer-row")).toBeOnTheScreen();
    expect(screen.getByText("Which database should the tests use?")).toBeOnTheScreen();
    expect(screen.queryByTestId("composer-status-line")).toBeNull();
    expect(screen.getByText("Do not type a password or a token here.")).toBeOnTheScreen();
    expect(screen.getByLabelText("Your answer").props.placeholder).toBe("Your answer");
    // A turn runs, but Send answers: no Stop and no Steer/Queue capsule.
    expect(screen.queryByRole("button", { name: "Stop the turn" })).toBeNull();
    expect(screen.getByRole("button", { name: "Send the answer" })).toBeDisabled();
  });

  it("sends the answer on one line, and the text waits until efrd takes it", async () => {
    const sent: string[] = [];
    const answer = answering(sent, { count: 0 }, { text: "Postgres 16,\non port 5433" });
    const harness = await renderComposer({ answering: answer, runningTurnId: "t1" });
    await fireEvent.press(screen.getByRole("button", { name: "Send the answer" }));
    expect(sent).toEqual(["Postgres 16, on port 5433"]);
    // The screen empties the field once efrd took the answer.
    expect(answer.draft.state).toBe("Postgres 16,\non port 5433");
    expect(harness.drafts.text(composerDraftKey(CONVERSATION))).toBe("");
    expect(harness.outbox.store.get().entries).toEqual([]);
  });

  it("keeps the typed answer out of the message draft", async () => {
    const answer = answering([], { count: 0 });
    const harness = await renderComposer({ answering: answer, text: "my own message" });
    expect(screen.getByLabelText("Your answer").props.value).toBe("");
    await fireEvent.changeText(screen.getByLabelText("Your answer"), "type npm publish");
    expect(answer.draft.state).toBe("type npm publish");
    // When the question ends elsewhere, the field shows the message draft again: the
    // words of the answer never become a message (ADR 0025).
    expect(harness.drafts.text(composerDraftKey(CONVERSATION))).toBe("my own message");
  });

  it("says not sent when the answer did not arrive, and keeps the text", async () => {
    await renderComposer({
      answering: answering([], { count: 0 }, { text: "SQLite", failed: true }),
    });
    expect(screen.getByTestId("composer-answer-not-sent")).toHaveTextContent("not sent");
    expect(screen.getByLabelText("Your answer").props.value).toBe("SQLite");
  });

  it("dims Send past 1024 bytes", async () => {
    await renderComposer({ answering: answering([], { count: 0 }, { text: "x".repeat(1025) }) });
    expect(screen.getByRole("button", { name: "Send the answer" })).toBeDisabled();
  });

  it("shows the braille cell while the answer is sent", async () => {
    await renderComposer({
      answering: answering([], { count: 0 }, { text: "SQLite", sending: true }),
    });
    expect(screen.getByTestId("composer-sending")).toBeOnTheScreen();
    expect(screen.queryByTestId("composer-answer-not-sent")).toBeNull();
  });

  it("leaves with the xmark", async () => {
    const left = { count: 0 };
    await renderComposer({ answering: answering([], left, { text: "maybe" }) });
    await fireEvent.press(screen.getByRole("button", { name: "Leave the answer" }));
    expect(left.count).toBe(1);
  });
});

/** The attachments of a test composer, kept so the test can prepare and pick. */
function withAttach(): {
  readonly make: (outbox: Outbox) => TestAttach;
  readonly get: () => TestAttach;
} {
  let made: TestAttach | undefined;
  return {
    make: (outbox) => {
      made = testAttach(outbox);
      return made;
    },
    get: () => {
      if (made === undefined) {
        throw new Error("the composer made no attachments");
      }
      return made;
    },
  };
}

/** Picks `files` in the picker of `source` through the plus menu. */
async function pick(test: TestAttach, source: string, files: TestAttach["picks"]): Promise<void> {
  test.picks.push(...files);
  await fireEvent(screen.getByTestId("composer-attach-menu"), "pressAction", {
    nativeEvent: { event: source },
  });
  await act(async () => {
    await Promise.resolve();
  });
}

describe("the files of the draft (S-Attach)", () => {
  it("opens the native menu of the plus with the three ways in, in the order of the board", async () => {
    await renderComposer({ attach: testAttach });
    const menu = screen.getByTestId("composer-attach-menu");
    expect(menu.props.shouldOpenOnLongPress).toBe(false);
    expect(
      menu.props.actions.map((action: { id: string; title: string; image: string }) => [
        action.id,
        action.title,
        action.image,
      ]),
    ).toEqual([
      ["photos", "Photo Library", "photo.on.rectangle"],
      ["camera", "Take Photo", "camera"],
      ["files", "Choose File", "folder"],
    ]);
  });

  it("puts the picked files in one row, keeps Send off while a photo prepares and with no text", async () => {
    const attach = withAttach();
    await renderComposer({ attach: attach.make });
    const test = attach.get();
    await pick(test, "photos", [
      { kind: "photo", uri: "file:///caches/IMG_1.HEIC", name: "IMG_1.HEIC" },
    ]);
    expect(test.picked).toEqual(["photos 10"]);
    expect(screen.getByLabelText("Photo, IMG_1.jpg, preparing")).toBeOnTheScreen();
    await fireEvent.changeText(input(), "Why is the header cut off?");
    expect(sendButton()).toBeDisabled();
    await act(async () => {
      test.files.finish("file:///caches/IMG_1.HEIC");
      await Promise.resolve();
    });
    expect(screen.getByLabelText("Photo, IMG_1.jpg, 1.2 MB")).toBeOnTheScreen();
    expect(sendButton()).toBeEnabled();
    await fireEvent.changeText(input(), "");
    expect(sendButton()).toBeDisabled();
  });

  it("sends the files with the prompt through the outbox, and the draft is empty after it", async () => {
    const attach = withAttach();
    const harness = await renderComposer({ attach: attach.make });
    const test = attach.get();
    await pick(test, "files", [
      { kind: "pdf", uri: "file:///caches/report.pdf", name: "report.pdf", bytes: 3_400_000 },
    ]);
    await fireEvent.changeText(input(), "Do the numbers match?");
    await fireEvent.press(sendButton());
    const entry = harness.outbox.store.get().entries[0];
    expect(entry?.attachments).toEqual([
      expect.objectContaining({ kind: "pdf", name: "report.pdf", bytes: 3_400_000, received: 0 }),
    ]);
    expect(screen.queryByTestId("composer-attachments")).toBeNull();
  });

  it("removes a tile with its xmark, and names the file in its label", async () => {
    const attach = withAttach();
    await renderComposer({ attach: attach.make });
    const test = attach.get();
    await pick(test, "files", [
      { kind: "file", uri: "file:///caches/data.csv", name: "data.csv", bytes: 48_000 },
    ]);
    const remove = screen.getByRole("button", { name: "Remove data.csv" });
    expect(remove.props.hitSlop).toBe((SPACING.hit - TILE.mark) / 2);
    await fireEvent.press(remove);
    expect(screen.queryByTestId("composer-attachments")).toBeNull();
    expect(test.files.removed).toEqual(["file:///caches/data.csv"]);
  });

  it("offers Quick Look and Remove on a long press of a photo, and only Remove for a file", async () => {
    const attach = withAttach();
    await renderComposer({ attach: attach.make });
    const test = attach.get();
    await pick(test, "photos", [{ kind: "photo", uri: "file:///caches/P.HEIC", name: "P.HEIC" }]);
    await act(async () => {
      test.files.finish("file:///caches/P.HEIC");
      await Promise.resolve();
    });
    await pick(test, "files", [
      { kind: "file", uri: "file:///caches/a.txt", name: "a.txt", bytes: 3 },
    ]);
    const [photoMenu, fileMenu] = screen.getAllByTestId(/^attachment-menu-/);
    const ids = (menu: typeof photoMenu) =>
      menu?.props.actions.map((action: { id: string }) => action.id);
    expect(ids(photoMenu)).toEqual(["look", "remove"]);
    expect(ids(fileMenu)).toEqual(["remove"]);
    expect(photoMenu?.props.shouldOpenOnLongPress).toBe(true);
    if (photoMenu !== undefined) {
      await fireEvent(photoMenu, "pressAction", { nativeEvent: { event: "look" } });
    }
    expect(test.files.looks).toEqual(["file:///caches/P.HEIC.jpg P.jpg"]);
  });

  it("names a file over the limit under the row and keeps Send off", async () => {
    const attach = withAttach();
    await renderComposer({ attach: attach.make, text: "Why does it crash?" });
    await pick(attach.get(), "files", [
      { kind: "file", uri: "file:///caches/logs.zip", name: "logs.zip", bytes: 34_000_000 },
    ]);
    expect(screen.getByText("logs.zip is 34 MB. The limit is 20 MB.")).toBeOnTheScreen();
    expect(screen.getByLabelText("File, logs.zip, 34 MB. The limit is 20 MB.")).toBeOnTheScreen();
    expect(sendButton()).toBeDisabled();
  });

  it("turns the plus off at the limit of efrd", async () => {
    const attach = withAttach();
    await renderComposer({ attach: attach.make });
    const files = Array.from({ length: LIMITS.max_per_message }, (_, i) => ({
      kind: "file" as const,
      uri: `file:///caches/${i}.txt`,
      name: `${i}.txt`,
      bytes: 1,
    }));
    await pick(attach.get(), "files", files);
    const plus = screen.getByRole("button", { name: "Attach" });
    expect(plus).toBeDisabled();
    expect(plus.props.accessibilityHint).toBe("10 attachments is the limit.");
    expect(screen.queryByTestId("composer-attach-menu")).toBeNull();
  });

  it("offers only Queue, with no menu, while a turn runs and the draft holds a file", async () => {
    const attach = withAttach();
    const harness = await renderComposer({
      attach: attach.make,
      runningTurnId: "t1",
      text: "And the footer",
    });
    await pick(attach.get(), "files", [
      { kind: "file", uri: "file:///caches/a.txt", name: "a.txt", bytes: 3 },
    ]);
    const capsule = screen.getByRole("button", { name: "Queue" });
    expect(capsule.props.accessibilityHint).toBe("A steer takes no attachment.");
    expect(screen.queryByTestId("composer-send-mode")).toBeNull();
    await fireEvent.press(capsule);
    expect(harness.caller.callsOf("turn.steer")).toHaveLength(0);
    expect(harness.outbox.store.get().entries[0]?.attachments).toHaveLength(1);
  });
});

/** The efr-speech of a composer that the setup gave voice. */
function speechOf(harness: Harness): FakeSpeech {
  if (harness.speech === undefined) {
    throw new Error("the composer has no voice");
  }
  return harness.speech;
}

describe("voice input (S-Voice)", () => {
  it("records into the draft at the cursor, with Send off, and never sends", async () => {
    const harness = await renderComposer({ voice: true, text: "Look at the header." });
    const speech = speechOf(harness);
    await fireEvent(input(), "selectionChange", {
      nativeEvent: { selection: { start: 19, end: 19 } },
    });
    await fireEvent.press(screen.getByRole("button", { name: "Record a voice message" }));
    expect(speech.calls).toEqual(["start"]);
    await act(async () => {
      speech.phase("recording");
    });
    expect(screen.getByRole("button", { name: "Stop recording, 0 seconds" })).toBeOnTheScreen();
    expect(screen.getByText("0:00", HIDDEN)).toBeOnTheScreen();
    expect(sendButton()).toBeDisabled();
    await act(async () => {
      speech.transcript("It is cut off.");
    });
    expect(input().props.value).toBe("Look at the header. It is cut off.");
    await fireEvent.press(screen.getByRole("button", { name: "Stop recording, 0 seconds" }));
    expect(speech.calls).toEqual(["start", "stop"]);
    await act(async () => {
      speech.end("stopped");
    });
    expect(screen.getByRole("button", { name: "Record a voice message" })).toBeOnTheScreen();
    expect(sendButton()).toBeEnabled();
    expect(harness.outbox.store.get().entries).toEqual([]);
  });

  it("says that iOS gets the speech model, in the slot of the microphone and above the line", async () => {
    const harness = await renderComposer({ voice: true });
    await fireEvent.press(screen.getByRole("button", { name: "Record a voice message" }));
    await act(async () => {
      speechOf(harness).phase("model");
    });
    expect(screen.getByText("Getting the speech model")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Getting the speech model" })).toBeDisabled();
  });

  it("opens the iOS settings from the line of a denied microphone, and Send takes a line away", async () => {
    const open = jest.spyOn(Linking, "openSettings").mockResolvedValue();
    const harness = await renderComposer({ voice: true, text: "Hi" });
    speechOf(harness).startError = MICROPHONE_DENIED;
    await fireEvent.press(screen.getByRole("button", { name: "Record a voice message" }));
    await act(async () => {
      await Promise.resolve();
    });
    const line = screen.getByRole("button", { name: VOICE_LINES.denied });
    await fireEvent.press(line);
    expect(open).toHaveBeenCalledTimes(1);
    await fireEvent.press(sendButton());
    expect(screen.queryByTestId("composer-voice-line")).toBeNull();
    open.mockRestore();
  });

  it("says that iOS stopped the recording, and keeps the text so far", async () => {
    const harness = await renderComposer({ voice: true });
    const speech = speechOf(harness);
    await fireEvent.press(screen.getByRole("button", { name: "Record a voice message" }));
    await act(async () => {
      speech.phase("recording");
      speech.transcript("so far");
      speech.end("interrupted");
    });
    expect(screen.getByText(VOICE_LINES.interrupted)).toBeOnTheScreen();
    expect(input().props.value).toBe("so far");
  });
});
