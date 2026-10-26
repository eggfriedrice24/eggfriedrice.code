import { describe, expect, it } from "vitest";

import {
  composerDraftKey,
  composerLink,
  composerState,
  inFlightText,
} from "@/features/conversation/model/composer-state";
import { selectDraft } from "@/features/conversation/model/drafts";
import { COMPOSER_CASES, composerEnvironment } from "./composer-cases";
import { MemoryKeyValue } from "./demo-services";

const NOW = Date.parse("2026-10-26T12:00:00Z");

/** The state that the composer of `name` derives, as `useComposer` does. */
function derived(name: string) {
  const one = COMPOSER_CASES.find((each) => each.name === name);
  if (one === undefined) {
    throw new Error(`no composer state ${name}`);
  }
  let ids = 0;
  const env = composerEnvironment(one, {
    storage: new MemoryKeyValue(),
    now: () => NOW,
    newId: () => {
      ids += 1;
      return `0199d3e0-de34-7000-8000-${ids.toString(16).padStart(12, "0")}`;
    },
    conversation: "0199d3e0-de34-7000-8000-0000000000aa",
  });
  return composerState({
    isBlankRoot: env.target.isBlankRoot,
    link: composerLink(undefined, env.connection.get().view),
    runningTurnId: one.running ? "0199d3e0-de34-7000-8000-0000000000bb" : undefined,
    text: selectDraft(composerDraftKey(env.target))(env.drafts.store.get()),
    sending: inFlightText(env.outbox.store.get(), env.target) !== undefined,
    sendMode: env.preferences.store.get().sendMode,
  });
}

describe("the states of the composer board", () => {
  it("give each state of the canvas through the composer's own functions", () => {
    const expected: Record<string, unknown> = {
      blank: { kind: "blank" },
      idle: { kind: "idle" },
      typing: { kind: "typing" },
      "typing at 6 lines": { kind: "typing" },
      "running, empty": { kind: "running", hasText: false, sendMode: "steer" },
      "running, typing (Steer)": { kind: "running", hasText: true, sendMode: "steer" },
      "steer menu open": { kind: "running", hasText: true, sendMode: "steer" },
      "running, Queue chosen": { kind: "running", hasText: true, sendMode: "queue" },
      sending: { kind: "sending", running: false },
      reconnecting: { kind: "reconnecting", running: false },
      "disconnected: offline": { kind: "disconnected", reason: "unreachable" },
      "disconnected: Tailscale is off": { kind: "disconnected", reason: "unreachable" },
      "disconnected: efrd is starting": { kind: "disconnected", reason: "starting" },
      "disconnected: re-pair needed": { kind: "disconnected", reason: "repair" },
      "disconnected: efrd too old": { kind: "disconnected", reason: "update" },
      "keyboard up": { kind: "typing" },
    };
    expect(COMPOSER_CASES.map((one) => one.name)).toEqual(Object.keys(expected));
    for (const [name, state] of Object.entries(expected)) {
      expect({ name, state: derived(name) }).toEqual({ name, state });
    }
  });
});
