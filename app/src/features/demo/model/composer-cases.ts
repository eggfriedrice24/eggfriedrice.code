// The states of the S-Composer board (design-spec section 1, "Composer"), each with what
// the real composer reads: the connection of its machine, its draft, the running turn, a
// prompt in flight and the send mode. `composerEnvironment` makes the stores of one
// state in memory; the composer then derives its state with the functions of the app
// (model/composer-state.ts), as on a real screen. The read-only state of the canvas is
// gone (plan 5.2 item x6). Pure.

import { createAtom, type Atom } from "@tanstack/store";

import type { ClientState } from "@/lib/net/client";
import { ConnectionError } from "@/lib/net/errors";
import type { MachineCaller } from "@/lib/query/machine-caller";
import type { KeyValueStorage } from "@/lib/store/key-value";
import { createDrafts, type Drafts } from "@/features/conversation/model/drafts";
import {
  composerDraftKey,
  type ComposerTarget,
} from "@/features/conversation/model/composer-state";
import { createOutbox, type Outbox } from "@/features/conversation/model/outbox";
import {
  initialConnection,
  type MachineConnection,
} from "@/features/machines/model/machine-connection";
import {
  createPreferences,
  type PreferencesStore,
  type SendMode,
} from "@/features/settings/model/preferences";
import { demoMachine } from "./demo-machines";
import { demoHello } from "./demo-server";

/** The connection of the composer's machine in a state. */
export type CaseLink = "ready" | "connecting" | "unreachable" | "starting" | "repair" | "update";

/** One state of the composer. */
export interface ComposerCase {
  readonly name: string;
  readonly blankRoot: boolean;
  readonly link: CaseLink;
  readonly text: string;
  readonly running: boolean;
  /** A prompt of this composer waits for efrd. */
  readonly sending: boolean;
  readonly sendMode: SendMode;
  readonly tailnetDown: boolean;
  /** What to do to see the state, when it needs a tap. */
  readonly hint?: string;
}

const TYPED = "fix the flaky sum test";

const SIX_LINES = [
  "fix the flaky sum test, and then:",
  "1. run the tests of efr-store",
  "2. run clippy on the workspace",
  "3. format the code",
  "4. write a line in the changelog",
  "5. tell me what changed",
].join("\n");

function state(name: string, fields: Partial<Omit<ComposerCase, "name">>): ComposerCase {
  const base: ComposerCase = {
    name,
    blankRoot: false,
    link: "ready",
    text: "",
    running: false,
    sending: false,
    sendMode: "steer",
    tailnetDown: false,
  };
  return { ...base, ...fields };
}

/** Every state of the board, in the order of the canvas. */
export const COMPOSER_CASES: readonly ComposerCase[] = [
  state("blank", { blankRoot: true }),
  state("idle", {}),
  state("typing", { text: TYPED }),
  state("typing at 6 lines", { text: SIX_LINES }),
  state("running, empty", { running: true }),
  state("running, typing (Steer)", { running: true, text: "only the store crate" }),
  state("steer menu open", {
    running: true,
    text: "only the store crate",
    hint: "Tap the Steer capsule.",
  }),
  state("running, Queue chosen", { running: true, text: "then run clippy", sendMode: "queue" }),
  state("sending", { text: "", sending: true }),
  state("reconnecting", { link: "connecting", text: TYPED }),
  state("disconnected: offline", { link: "unreachable" }),
  state("disconnected: Tailscale is off", { link: "unreachable", tailnetDown: true }),
  state("disconnected: efrd is starting", { link: "starting" }),
  state("disconnected: re-pair needed", { link: "repair" }),
  state("disconnected: efrd too old", { link: "update" }),
  state("keyboard up", { text: TYPED, hint: "Tap the field." }),
];

/** The client state of `link`, at clock time `now`. */
function clientOf(link: CaseLink, now: number): ClientState {
  const machine = demoMachine("desktop");
  switch (link) {
    case "ready":
      return {
        state: "ready",
        hello: demoHello(machine, 1),
        scopes: ["read", "operate", "approve"],
      };
    case "connecting":
      return { state: "connecting" };
    case "unreachable":
      return {
        state: "waiting",
        retryAt: now + 30_000,
        error: new ConnectionError("socket", "the socket closed (1006)"),
      };
    case "starting":
      return {
        state: "waiting",
        retryAt: now + 3000,
        error: new ConnectionError("starting", "efrd is starting", { retryAfterMs: 3000 }),
      };
    case "repair":
      return {
        state: "stopped",
        error: new ConnectionError("proof_refused", "this device is not known", { rePair: true }),
      };
    default:
      return {
        state: "stopped",
        error: new ConnectionError("protocol_mismatch", "efrd speaks protocol 0", {
          daemonProtocol: 0,
        }),
      };
  }
}

/** What one composer of the board reads. */
export interface ComposerEnvironment {
  readonly target: ComposerTarget;
  readonly connection: Atom<MachineConnection>;
  readonly drafts: Drafts;
  readonly outbox: Outbox;
  readonly preferences: PreferencesStore;
}

/** What the stores of the board need. */
export interface EnvironmentDeps {
  /** A store for the drafts and the preferences of this state. */
  readonly storage: KeyValueStorage;
  readonly now: () => number;
  readonly newId: () => string;
  /** The conversation of the composer, or the local id of the blank root. */
  readonly conversation: string;
}

/** The stores of `one`: its draft, its prompt in flight, its send mode and its link. */
export function composerEnvironment(one: ComposerCase, deps: EnvironmentDeps): ComposerEnvironment {
  const machine = demoMachine("desktop");
  const target: ComposerTarget = {
    machine: machine.daemonId,
    id: deps.conversation,
    isBlankRoot: one.blankRoot,
  };
  const drafts = createDrafts({ storage: deps.storage });
  if (one.text !== "") {
    drafts.set(composerDraftKey(target), one.text);
  }
  const outbox = createOutbox({ now: deps.now, newId: deps.newId });
  if (one.sending) {
    outbox.enqueue({
      machine: machine.daemonId,
      target: { kind: "conversation", conversationId: deps.conversation },
      text: "run the tests again",
    });
  }
  const preferences = createPreferences({ storage: deps.storage });
  preferences.setSendMode(one.sendMode);
  const connection = createAtom(
    initialConnection(clientOf(one.link, deps.now()), `${machine.host}:${machine.port}`),
  );
  return { target, connection, drafts, outbox, preferences };
}

/** A caller whose calls never end, so the buttons of the board change nothing. */
export const QUIET_CALLER: MachineCaller = {
  call: () => new Promise(() => undefined),
};
