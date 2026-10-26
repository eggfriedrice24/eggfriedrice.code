// The session of the screens, given by the root layout. Until the root layout passes
// the session of the store, the screens get `UNPAIRED`: a session with no device key,
// whose pairing flow says that this build cannot pair.

import { createContext, type ReactNode, useContext } from "react";

import { EMPTY_STREAM } from "../core/reducer";
import { EMPTY_DETAILS } from "./details";
import { createValueStore } from "./external_store";
import type { PairingView, PhoneSession } from "./session";

const pairingState = createValueStore<PairingView>({ kind: "idle" });
const EMPTY_STREAM_STORE = createValueStore(EMPTY_STREAM);
const EMPTY_DETAILS_STORE = createValueStore(EMPTY_DETAILS);
const NO_TITLE = createValueStore<string | undefined>(undefined);

/** A session that is not paired and cannot pair: the default of the context. */
export const UNPAIRED: PhoneSession = {
  connection: createValueStore({ kind: "unpaired" }),
  conversations: createValueStore([]),
  pairing: {
    state: pairingState,
    start: () =>
      pairingState.set({
        kind: "failed",
        reason: "other",
        message: "This build of the app cannot pair yet.",
      }),
    cancel: () => pairingState.set({ kind: "idle" }),
  },
  conversation: (conversationId) => ({
    id: conversationId,
    stream: EMPTY_STREAM_STORE,
    details: EMPTY_DETAILS_STORE,
    title: NO_TITLE,
  }),
  sendPrompt: () => Promise.resolve({ ok: false, message: "The phone is not paired." }),
  answerApproval: () => Promise.resolve({ ok: false, message: "The phone is not paired." }),
  reconnect: () => {},
  forget: () => Promise.resolve(),
};

const SessionContext = createContext<PhoneSession>(UNPAIRED);

const DemoContext = createContext<(() => void) | undefined>(undefined);

/**
 * Gives `session` to the screens below. `onDemo`, given only in a dev build, swaps in the
 * demo session (demo_session.ts).
 */
export function SessionProvider(props: {
  readonly session: PhoneSession;
  readonly onDemo?: () => void;
  readonly children: ReactNode;
}) {
  return (
    <SessionContext value={props.session}>
      <DemoContext value={props.onDemo}>{props.children}</DemoContext>
    </SessionContext>
  );
}

/** The session of the screens. */
export function useSession(): PhoneSession {
  return useContext(SessionContext);
}

/** The switch to the demo session, in a dev build only. */
export function useDemo(): (() => void) | undefined {
  return useContext(DemoContext);
}
