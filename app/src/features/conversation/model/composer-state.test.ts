import { describe, expect, it } from "vitest";

import type { ConnectionView } from "@/features/machines/model/connection-view";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import { EMPTY_STREAM } from "@/lib/stream/reducer";
import {
  BLANK_STATUS,
  capsuleLabel,
  composerDraftKey,
  composerEditable,
  composerLink,
  composerPlaceholder,
  composerState,
  composerTrailing,
  conversationIdOf,
  createLinkSelector,
  disconnectText,
  hasText,
  ignoreReason,
  inFlightText,
  outboxTargetOf,
  SEND_MODE_CHOICES,
  selectComposerStatus,
  statusParts,
  type ComposerInput,
  type ComposerLink,
  type ComposerState,
  type ComposerTarget,
} from "./composer-state";
import { CONVERSATION_A, DESKTOP, LAPTOP, testOutbox } from "./outbox-testing";

const ENDPOINT = "100.64.0.1:7878";
const ONLINE: ConnectionView = { kind: "online", endpoint: ENDPOINT };
const CONNECTING: ConnectionView = { kind: "connecting", endpoint: ENDPOINT };
const RETRYING: ConnectionView = { kind: "retrying", endpoint: ENDPOINT, retryAt: 5 };
const UNREACHABLE: ConnectionView = { kind: "unreachable", endpoint: ENDPOINT, retryAt: 5 };
const STARTING: ConnectionView = { kind: "starting", endpoint: ENDPOINT, retryAt: 5 };

const online = composerLink(undefined, ONLINE);

function input(change: Partial<ComposerInput>): ComposerInput {
  return {
    isBlankRoot: false,
    link: online,
    runningTurnId: undefined,
    text: "",
    sending: false,
    sendMode: "steer",
    ...change,
  };
}

const WORDS = { machineName: "laptop", tailnetDown: false };

function connection(view: ConnectionView): MachineConnection {
  return { client: { state: "off" }, view, seenAt: undefined };
}

describe("the link of the composer", () => {
  it("is online, reconnecting or disconnected by the connection", () => {
    expect(composerLink(undefined, ONLINE)).toEqual({ kind: "online" });
    expect(composerLink(undefined, CONNECTING)).toEqual({ kind: "reconnecting" });
    expect(composerLink(online, RETRYING)).toEqual({ kind: "reconnecting" });
    expect(composerLink(online, UNREACHABLE)).toEqual({
      kind: "disconnected",
      reason: "unreachable",
    });
    expect(composerLink(online, STARTING)).toEqual({ kind: "disconnected", reason: "starting" });
    expect(composerLink(online, { kind: "repair_needed", reason: "unknown_device" })).toEqual({
      kind: "disconnected",
      reason: "repair",
    });
    expect(composerLink(online, { kind: "unpaired" })).toEqual({
      kind: "disconnected",
      reason: "repair",
    });
    expect(composerLink(online, { kind: "update_needed" })).toEqual({
      kind: "disconnected",
      reason: "update",
    });
  });

  it("stays disconnected while the client tries a machine that does not answer", () => {
    const down = composerLink(online, UNREACHABLE);
    expect(composerLink(down, CONNECTING)).toBe(down);
    expect(composerLink(down, RETRYING)).toBe(down);
    expect(composerLink(down, ONLINE)).toEqual({ kind: "online" });
  });

  it("gives the same object for the same link, so a compare with === sees no change", () => {
    expect(composerLink(undefined, CONNECTING)).toBe(composerLink(online, RETRYING));
    expect(composerLink(undefined, UNREACHABLE)).toBe(composerLink(online, UNREACHABLE));
  });

  it("keeps the last link in a selector of the connection atom", () => {
    const select = createLinkSelector();
    const links: ComposerLink["kind"][] = [ONLINE, RETRYING, UNREACHABLE, CONNECTING, ONLINE].map(
      (view) => select(connection(view)).kind,
    );
    expect(links).toEqual(["online", "reconnecting", "disconnected", "disconnected", "online"]);
    // A second call with the same value gives the same link (useSyncExternalStore can do it).
    expect(select(connection(ONLINE))).toBe(select(connection(ONLINE)));
  });
});

describe("the state of the composer", () => {
  it("is blank on the blank root and idle in a conversation, with an empty field", () => {
    expect(composerState(input({ isBlankRoot: true }))).toEqual({ kind: "blank" });
    expect(composerState(input({}))).toEqual({ kind: "idle" });
  });

  it("is typing with text, and whitespace is no text", () => {
    expect(composerState(input({ text: "Now write the migration notes" }))).toEqual({
      kind: "typing",
    });
    expect(composerState(input({ isBlankRoot: true, text: "hi" }))).toEqual({ kind: "typing" });
    expect(composerState(input({ text: " \n\t" }))).toEqual({ kind: "idle" });
    expect(hasText(" a ")).toBe(true);
  });

  it("is running while a turn runs, with the send mode", () => {
    expect(composerState(input({ runningTurnId: "t1" }))).toEqual({
      kind: "running",
      hasText: false,
      sendMode: "steer",
    });
    expect(composerState(input({ runningTurnId: "t1", text: "Use TurnState" }))).toEqual({
      kind: "running",
      hasText: true,
      sendMode: "steer",
    });
    expect(
      composerState(input({ runningTurnId: "t1", text: "Also a test", sendMode: "queue" })),
    ).toEqual({ kind: "running", hasText: true, sendMode: "queue" });
  });

  it("does not run on the blank root", () => {
    expect(composerState(input({ isBlankRoot: true, runningTurnId: "t1" }))).toEqual({
      kind: "blank",
    });
  });

  it("is sending while a send waits for efrd, and keeps Stop of a running turn", () => {
    expect(composerState(input({ sending: true, text: "x" }))).toEqual({
      kind: "sending",
      running: false,
    });
    expect(composerState(input({ sending: true, runningTurnId: "t1" }))).toEqual({
      kind: "sending",
      running: true,
    });
  });

  it("puts the connection first", () => {
    const reconnecting = composerLink(undefined, CONNECTING);
    const down = composerLink(undefined, UNREACHABLE);
    expect(composerState(input({ link: reconnecting, sending: true, text: "x" }))).toEqual({
      kind: "reconnecting",
      running: false,
    });
    expect(composerState(input({ link: reconnecting, runningTurnId: "t1" }))).toEqual({
      kind: "reconnecting",
      running: true,
    });
    expect(composerState(input({ link: down, runningTurnId: "t1", text: "x" }))).toEqual({
      kind: "disconnected",
      reason: "unreachable",
    });
    expect(composerState(input({ link: down, isBlankRoot: true }))).toEqual({
      kind: "disconnected",
      reason: "unreachable",
    });
  });
});

describe("the trailing action", () => {
  const cases: [ComposerState, ReturnType<typeof composerTrailing>][] = [
    [{ kind: "blank" }, { kind: "send", enabled: false }],
    [{ kind: "idle" }, { kind: "send", enabled: false }],
    [{ kind: "typing" }, { kind: "send", enabled: true }],
    [
      { kind: "running", hasText: false, sendMode: "steer" },
      { kind: "stop", capsule: null },
    ],
    [
      { kind: "running", hasText: true, sendMode: "steer" },
      { kind: "stop", capsule: "steer" },
    ],
    [
      { kind: "running", hasText: true, sendMode: "queue" },
      { kind: "stop", capsule: "queue" },
    ],
    [
      { kind: "sending", running: false },
      { kind: "sending", stop: false },
    ],
    [
      { kind: "sending", running: true },
      { kind: "sending", stop: true },
    ],
    [
      { kind: "reconnecting", running: true },
      { kind: "send", enabled: false },
    ],
    [
      { kind: "disconnected", reason: "starting" },
      { kind: "send", enabled: false },
    ],
  ];
  it.each(cases)("of %j is %j", (state, trailing) => {
    expect(composerTrailing(state)).toEqual(trailing);
  });

  it("lets the input take text except while it sends or has no connection", () => {
    expect(composerEditable({ kind: "reconnecting", running: false })).toBe(true);
    expect(composerEditable({ kind: "running", hasText: true, sendMode: "queue" })).toBe(true);
    expect(composerEditable({ kind: "sending", running: false })).toBe(false);
    expect(composerEditable({ kind: "disconnected", reason: "repair" })).toBe(false);
  });
});

describe("the placeholder", () => {
  it("says Message efr, or Steer or queue while a turn runs", () => {
    expect(composerPlaceholder({ kind: "blank" }, WORDS)).toBe("Message efr");
    expect(composerPlaceholder({ kind: "idle" }, WORDS)).toBe("Message efr");
    expect(composerPlaceholder({ kind: "running", hasText: false, sendMode: "steer" }, WORDS)).toBe(
      "Steer or queue",
    );
    expect(composerPlaceholder({ kind: "reconnecting", running: false }, WORDS)).toBe(
      "Message efr",
    );
    expect(composerPlaceholder({ kind: "reconnecting", running: true }, WORDS)).toBe(
      "Steer or queue",
    );
  });

  it("gives the short reason of a disconnected composer", () => {
    expect(composerPlaceholder({ kind: "disconnected", reason: "unreachable" }, WORDS)).toBe(
      "laptop is offline",
    );
    expect(disconnectText("unreachable", { machineName: "laptop", tailnetDown: true })).toBe(
      "Tailscale is not connected",
    );
    expect(disconnectText("starting", WORDS)).toBe("efrd is starting");
    expect(disconnectText("repair", WORDS)).toBe("re-pair needed");
    expect(disconnectText("update", WORDS)).toBe("update needed");
  });
});

describe("the target of a send", () => {
  const conversation: ComposerTarget = { machine: DESKTOP, id: CONVERSATION_A, isBlankRoot: false };

  it("keeps one draft per conversation, and one for the blank root of each machine", () => {
    expect(composerDraftKey(conversation)).toBe(`${DESKTOP}:${CONVERSATION_A}`);
    expect(composerDraftKey({ machine: DESKTOP, id: "local-1", isBlankRoot: true })).toBe(
      `${DESKTOP}:new`,
    );
    expect(composerDraftKey({ machine: DESKTOP, id: "local-2", isBlankRoot: true })).toBe(
      `${DESKTOP}:new`,
    );
  });

  it("is a new conversation on the blank root, and the conversation elsewhere", () => {
    const { outbox } = testOutbox();
    const snapshot = outbox.store.get();
    expect(outboxTargetOf(snapshot, { machine: DESKTOP, id: "local", isBlankRoot: true })).toEqual({
      kind: "new",
      localId: "local",
    });
    expect(outboxTargetOf(snapshot, conversation)).toEqual({
      kind: "conversation",
      conversationId: CONVERSATION_A,
    });
    expect(conversationIdOf(snapshot, conversation)).toBe(CONVERSATION_A);
  });

  it("stays new on a screen of a local id until efrd makes it, then is the conversation", () => {
    const { outbox } = testOutbox();
    const target = outbox.newConversation();
    if (target.kind !== "new") {
      throw new Error("a new target");
    }
    const screen: ComposerTarget = { machine: DESKTOP, id: target.localId, isBlankRoot: false };
    const first = outbox.enqueue({ machine: DESKTOP, target, text: "first" });
    expect(outboxTargetOf(outbox.store.get(), screen)).toEqual(target);
    expect(conversationIdOf(outbox.store.get(), screen)).toBe(target.localId);
    outbox.begin(first.commandId);
    outbox.succeed(first.commandId, {
      conversation_id: CONVERSATION_A,
      turn_id: "t1",
      seq: 3,
      queued: false,
    });
    expect(outboxTargetOf(outbox.store.get(), screen)).toEqual({
      kind: "conversation",
      conversationId: CONVERSATION_A,
    });
    expect(conversationIdOf(outbox.store.get(), screen)).toBe(CONVERSATION_A);
  });

  it("shows the newest prompt that waits for efrd, and none once efrd took it", () => {
    const { outbox } = testOutbox();
    const target = { kind: "conversation", conversationId: CONVERSATION_A } as const;
    expect(inFlightText(outbox.store.get(), conversation)).toBeUndefined();
    const one = outbox.enqueue({ machine: DESKTOP, target, text: "one" });
    outbox.enqueue({ machine: DESKTOP, target, text: "two" });
    outbox.enqueue({ machine: LAPTOP, target, text: "other machine" });
    expect(inFlightText(outbox.store.get(), conversation)).toBe("two");
    outbox.begin(one.commandId);
    outbox.succeed(one.commandId, {
      conversation_id: CONVERSATION_A,
      turn_id: "t1",
      seq: 3,
      queued: true,
    });
    expect(inFlightText(outbox.store.get(), conversation)).toBe("two");
    const two = outbox.store.get().entries.find((entry) => entry.text === "two");
    if (two === undefined) {
      throw new Error("the second entry");
    }
    outbox.begin(two.commandId);
    outbox.fail(two.commandId, { code: "invalid", message: "no" });
    expect(inFlightText(outbox.store.get(), conversation)).toBeUndefined();
  });

  it("shows the prompt of the blank root by its local id", () => {
    const { outbox } = testOutbox();
    const target = outbox.newConversation();
    if (target.kind !== "new") {
      throw new Error("a new target");
    }
    outbox.enqueue({ machine: DESKTOP, target, text: "hello" });
    const root: ComposerTarget = { machine: DESKTOP, id: target.localId, isBlankRoot: true };
    expect(inFlightText(outbox.store.get(), root)).toBe("hello");
  });
});

describe("the status line", () => {
  it("shows the cwd, the mode and the context", () => {
    expect(
      statusParts({
        cwd: "/home/egg/code/efr",
        mode: "cautious",
        context: { tokens: 42_000, limit: 100_000, window: 100_000 },
        waiting: "none",
      }),
    ).toEqual([
      { key: "cwd", text: "~/code/efr", tone: "text_muted" },
      { key: "mode", text: "cautious", tone: "text_subtle" },
      { key: "context", text: "ctx 42%", tone: "text_subtle" },
    ]);
  });

  it("adds waiting for you in yolk, or the terminal in grey", () => {
    const base = { cwd: undefined, mode: undefined, context: undefined } as const;
    expect(statusParts({ ...base, waiting: "phone" })).toEqual([
      { key: "waiting", text: "waiting for you", tone: "accent_primary" },
    ]);
    expect(statusParts({ ...base, waiting: "computer" })).toEqual([
      { key: "waiting", text: "waiting in the terminal", tone: "text_muted" },
    ]);
    expect(statusParts({ ...base, waiting: "none" })).toEqual([]);
  });

  it("shows the cwd and the mode of a new conversation on the blank root, with no ctx", () => {
    expect(statusParts(BLANK_STATUS)).toEqual([
      { key: "cwd", text: "~", tone: "text_muted" },
      { key: "mode", text: "cautious", tone: "text_subtle" },
    ]);
  });

  it("reads the stream store", () => {
    expect(selectComposerStatus(EMPTY_STREAM)).toEqual({
      cwd: undefined,
      mode: undefined,
      context: undefined,
      waiting: "none",
    });
  });
});

describe("the Steer/Queue menu", () => {
  it("has Steer now and Queue with their subtitles and glyphs", () => {
    expect(SEND_MODE_CHOICES).toEqual([
      {
        mode: "steer",
        title: "Steer now",
        subtitle: "Joins the running turn",
        image: "arrow.turn.left.up",
      },
      {
        mode: "queue",
        title: "Queue",
        subtitle: "Sends when this turn ends",
        image: "list.number",
      },
    ]);
    expect(capsuleLabel("steer")).toBe("Steer");
    expect(capsuleLabel("queue")).toBe("Queue");
  });
});

describe("the reason of an ignored submit", () => {
  it("names the state that sends nothing", () => {
    expect(ignoreReason({ kind: "disconnected", reason: "unreachable" })).toBe(
      "the machine is disconnected (unreachable)",
    );
    expect(ignoreReason({ kind: "reconnecting", running: false })).toBe("the connection opens");
    expect(ignoreReason({ kind: "sending", running: true })).toBe(
      "a prompt or a steer waits for efrd",
    );
    expect(ignoreReason({ kind: "idle" })).toBe("the field is empty (idle)");
  });
});
