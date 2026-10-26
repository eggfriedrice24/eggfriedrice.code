// The demo session of dev builds: a PhoneSession that reaches no computer. It plays the
// scripted stream of scenarios.ts in real time through the same two reducers that the
// store runs (the stream reducer of the core and the details reducer), so the
// transcript can be checked on a phone before pairing: the pacer, long replies, code
// blocks, tables, emoji and CJK text, reasoning, tool output and an approval question
// that waits for the user's answer. A prompt from the composer gets a short scripted
// reply.
//
// It is also the example of how the store wires a subscribe stream to the screens.
// The timers are a parameter, so the tests run it on a fake clock.

import { decodeSubscribeItem } from "@/lib/protocol/envelope";
import { EMPTY_STREAM, reduce } from "@/lib/stream/reducer";
import type { PhoneDecision } from "./approval";
import { EMPTY_DETAILS, reduceDetails } from "./details";
import { createValueStore } from "@/lib/store/frame-store";
import {
  APPROVAL_CALL,
  DEMO_APPROVAL_TURN,
  DEMO_CONVERSATION,
  demoScript,
  finishApproval,
  type Script,
  type TimedItem,
} from "./scenarios";
import type { ConversationItem, ConversationSource, PairingView, PhoneSession } from "./session";

/** The clock of the demo. */
export interface DemoTimers {
  /** Runs `callback` after `ms`; the result cancels it. */
  readonly schedule: (callback: () => void, ms: number) => () => void;
  /** The wall clock, in ms since the epoch. */
  readonly now: () => number;
}

/** A demo session, and a way to stop its timers. */
export interface DemoSession extends PhoneSession {
  readonly dispose: () => void;
}

const DEMO_REPLY =
  "This is the demo of the app. It plays scripted replies and does not reach your computer. To use efr from this phone, run `efr device add` on your computer and pair the phone.";

/** A demo session on `timers`. */
export function createDemoSession(timers: DemoTimers): DemoSession {
  const script: Script = demoScript();
  const stream = createValueStore(EMPTY_STREAM);
  const details = createValueStore(EMPTY_DETAILS);
  const title = createValueStore<string | undefined>("Demo of the transcript");
  const conversations = createValueStore<readonly ConversationItem[]>([
    {
      id: DEMO_CONVERSATION,
      title: "Demo of the transcript",
      status: "idle",
      updatedAt: timers.now(),
      cwd: "/home/me/p/efr",
    },
  ]);
  const pairing = createValueStore<PairingView>({ kind: "idle" });
  const source = { id: DEMO_CONVERSATION, stream, details, title };
  const others = new Map<string, ConversationSource>();
  const other = (id: string): ConversationSource => {
    const known = others.get(id);
    if (known !== undefined) {
      return known;
    }
    const empty: ConversationSource = {
      id,
      stream: createValueStore(EMPTY_STREAM),
      details: createValueStore(EMPTY_DETAILS),
      title: createValueStore<string | undefined>(undefined),
    };
    others.set(id, empty);
    return empty;
  };

  let items: readonly TimedItem[] = [];
  let next = 0;
  let timer: (() => void) | undefined;
  let started = false;
  let waiting = false;
  let answered = false;
  // Script time `baseScript` plays at wall clock time `baseWall`.
  let baseScript = 0;
  let baseWall = 0;
  let prompts = 0;

  const setStatus = (status: string) => {
    conversations.update((list) =>
      list.map((item) =>
        item.id === DEMO_CONVERSATION ? { ...item, status, updatedAt: timers.now() } : item,
      ),
    );
  };

  const apply = (item: unknown) => {
    const stamped = stamp(item, timers.now());
    details.update((state) => reduceDetails(state, stamped));
    const decoded = decodeSubscribeItem(stamped);
    if (decoded.ok) {
      stream.update((state) => reduce(state, decoded.value));
    }
    if (isQuestion(item) && !answered) {
      waiting = true;
    }
  };

  const pump = () => {
    timer = undefined;
    const now = timers.now();
    while (next < items.length) {
      // `apply` sets `waiting` when the question of the script comes.
      if (waiting) {
        break;
      }
      const due = baseWall + ((items[next]?.at ?? 0) - baseScript);
      if (due > now) {
        timer = timers.schedule(pump, due - now);
        setStatus("running");
        return;
      }
      apply(items[next]?.item);
      next++;
    }
    setStatus(waiting ? "awaiting_approval" : "idle");
  };

  /** Plays the items that the script has now and the ones it gets later. */
  const play = () => {
    const before = items.length;
    items = script.build("demo").items;
    if (timer === undefined && !waiting && next >= before) {
      baseScript = items[next]?.at ?? script.time;
      baseWall = timers.now();
    }
    if (timer === undefined) {
      pump();
    }
  };

  return {
    connection: createValueStore({ kind: "online", endpoint: "demo", daemonName: "demo" }),
    conversations,
    pairing: {
      state: pairing,
      start: () =>
        pairing.set({
          kind: "failed",
          reason: "other",
          message: "The demo cannot pair. Close the app and open it again to pair.",
        }),
      cancel: () => pairing.set({ kind: "idle" }),
    },
    conversation: (conversationId) => {
      // The demo starts when its conversation is first asked for, and plays on while
      // the app runs.
      if (conversationId === DEMO_CONVERSATION && !started) {
        started = true;
        play();
      }
      return conversationId === DEMO_CONVERSATION ? source : other(conversationId);
    },
    sendPrompt: (conversationId, text) => {
      if (conversationId !== DEMO_CONVERSATION) {
        return Promise.resolve({ ok: false, message: "The demo has one conversation only." });
      }
      prompts += 1;
      const turnId = `019a9b1c-3d00-7a10-8b20-0000000003${String(prompts).padStart(2, "0")}`;
      script
        .wait(200)
        .prompt(turnId, text, "phone")
        .wait(400)
        .say(turnId, 0, DEMO_REPLY)
        .done(turnId);
      play();
      return Promise.resolve({ ok: true });
    },
    answerApproval: (conversationId, callId, decision: PhoneDecision) => {
      if (conversationId !== DEMO_CONVERSATION || callId !== APPROVAL_CALL || !waiting) {
        return Promise.resolve({ ok: false, message: "This question cannot be answered now." });
      }
      answered = true;
      waiting = false;
      finishApproval(script.wait(300), DEMO_APPROVAL_TURN, decision, "phone");
      play();
      return Promise.resolve({ ok: true });
    },
    reconnect: () => {},
    forget: () => Promise.resolve(),
    dispose: () => {
      timer?.();
      timer = undefined;
    },
  };
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isQuestion(item: unknown): boolean {
  return (
    isRecord(item) && isRecord(item["event"]) && item["event"]["kind"] === "approval_requested"
  );
}

/** The item with the time of an event set to `now`, so a countdown counts from now. */
function stamp(item: unknown, now: number): unknown {
  if (!isRecord(item) || item["kind"] !== "event") {
    return item;
  }
  return Object.assign({}, item, { at: new Date(now).toISOString() });
}
