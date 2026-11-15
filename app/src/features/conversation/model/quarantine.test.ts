// The quarantine question with the reviewer off (Q-Quarantine 1 to 6): the reducer keeps
// it on the tool call, and the strip, the sheet and the card say it in efr's words.

import { describe, expect, it } from "vitest";

import type { SurfaceChange } from "@/lib/protocol/generated";
import type { Event, EventOf, SubscribeItem } from "@/lib/protocol/wire";
import { findQuarantine, pendingQuarantine, type StreamState } from "@/lib/stream/reducer";
import {
  eventItem,
  run,
  toolCallCompleted,
  toolCallStarted,
  turnCompleted,
  turnStarted,
} from "@/lib/stream/testing";
import { quarantineAnswer, quarantineLine, quarantineSheet, quarantineStrip } from "./quarantine";
import { selectComposerStatus, statusParts } from "./composer-state";
import { toolLabel, toolStatus, toolHeader } from "./tool-block";

const QUESTION = "019a9b1c-3d00-7a10-8b20-0000000000a1";
const CWD = "/home/me";

const FSMONITOR: SurfaceChange = {
  path: "/home/me/.git/config",
  rule: "config_code_key",
  key: "core.fsmonitor",
  quarantined: true,
};
const HOOK: SurfaceChange = {
  path: "/home/me/.git/hooks/pre-commit",
  rule: "hook_planted",
  quarantined: true,
};
const ENVRC: SurfaceChange = {
  path: "/home/me/.envrc",
  rule: "protected_name_created",
  quarantined: false,
};

let seq = 0;
const event = (body: Event): SubscribeItem => eventItem(++seq, body);

function asked(changes: readonly SurfaceChange[]): EventOf<"surface_question_requested"> {
  return {
    kind: "surface_question_requested",
    turn_id: "t1",
    call_id: "c1",
    question_id: QUESTION,
    changes: [...changes],
  };
}

function answered(
  keep: boolean,
  origin?: EventOf<"surface_question_answered">["origin"],
): EventOf<"surface_question_answered"> {
  const body: EventOf<"surface_question_answered"> = {
    kind: "surface_question_answered",
    turn_id: "t1",
    question_id: QUESTION,
    keep,
  };
  if (origin !== undefined) {
    body.origin = origin;
  }
  return body;
}

function stateOf(...bodies: Event[]): StreamState {
  return run([
    event(turnStarted("t1")),
    event(toolCallStarted("t1", "c1", "shell", { command: "pre-commit install" })),
    event(toolCallCompleted("t1", "c1", "installed")),
    ...bodies.map(event),
  ]);
}

function questionOf(state: StreamState) {
  const question = findQuarantine(state, "c1");
  if (question === undefined) {
    throw new Error("no question");
  }
  return question;
}

describe("the quarantine question", () => {
  it("waits on the card of its call, and the phone can answer it", () => {
    const state = stateOf(asked([FSMONITOR]));
    const question = questionOf(state);
    expect(question.answer).toBe("pending");
    expect(pendingQuarantine(state)).toEqual([question]);
    const call = state.turns.get("t1")?.tools.get("c1");
    if (call === undefined) {
      throw new Error("no call");
    }
    expect(toolStatus(call, false, 0)).toEqual({ kind: "settings" });
    expect(toolLabel(toolHeader(call), toolStatus(call, false, 0))).toBe(
      "Command pre-commit install, git settings changed, waiting for you",
    );
    // The phone can answer Leave (Q-Quarantine 8), so the question waits for the user here.
    expect(statusParts(selectComposerStatus(state, 0)).at(-1)).toEqual({
      key: "waiting",
      text: "waiting for you",
      tone: "accent_primary",
    });
  });

  it("asks about one change by its path, and about many by their count", () => {
    expect(quarantineLine(questionOf(stateOf(asked([FSMONITOR]))), CWD)).toBe(
      "keep .git/config (core.fsmonitor)?",
    );
    expect(quarantineLine(questionOf(stateOf(asked([FSMONITOR, HOOK]))), CWD)).toBe(
      "keep 2 git changes?",
    );
    expect(quarantineStrip(questionOf(stateOf(asked([FSMONITOR]))), CWD, "desktop")).toEqual({
      callId: "c1",
      questionId: QUESTION,
      line: "keep .git/config (core.fsmonitor)?",
      label: "Git settings changed: keep .git/config (core.fsmonitor)?",
      keepElsewhere: "Keep on desktop",
      keepLabel: "Keep is possible only on desktop",
    });
  });

  it("lists only the changes that went to quarantine in the sheet", () => {
    const question = questionOf(stateOf(asked([FSMONITOR, HOOK, ENVRC])));
    expect(quarantineSheet(question, CWD, "desktop", false)).toEqual({
      rows: [
        { path: ".git/config", what: "core.fsmonitor" },
        { path: ".git/hooks/pre-commit", what: "a new hook" },
      ],
      quiet: "Git settings that run programs, moved to quarantine, where nothing reads them.",
      questionId: QUESTION,
      keepElsewhere: "Keep on desktop",
      keepLabel: "Keep is possible only on desktop",
      closed: undefined,
    });
  });

  it("says how it ended: kept, left, or not answered", () => {
    const left = stateOf(asked([FSMONITOR]), answered(false, "shell"));
    const call = left.turns.get("t1")?.tools.get("c1");
    if (call === undefined) {
      throw new Error("no call");
    }
    expect(toolStatus(call, false, 0)).toEqual({ kind: "quarantined", word: "left in quarantine" });
    expect(pendingQuarantine(left)).toEqual([]);
    expect(quarantineSheet(questionOf(left), CWD, "desktop", false)).toMatchObject({
      keepElsewhere: undefined,
      closed: "left in quarantine · on desktop",
    });
    const kept = stateOf(asked([FSMONITOR]), answered(true, "shell"));
    expect(quarantineAnswer(questionOf(kept), false, "desktop")).toBe("kept · on desktop");
    // No origin: the question expired or the turn was interrupted.
    const expired = stateOf(asked([FSMONITOR]), answered(false));
    expect(questionOf(expired).answer).toBe("not_answered");
    expect(quarantineAnswer(questionOf(expired), false, "desktop")).toBe(
      "not answered · left in quarantine",
    );
    // The first answer wins.
    const twice = stateOf(asked([FSMONITOR]), answered(false, "shell"), answered(true, "cli"));
    expect(questionOf(twice).answer).toBe("left");
  });

  it("closes a question that waits when its turn ends", () => {
    const state = stateOf(asked([FSMONITOR]), turnCompleted("t1"));
    expect(questionOf(state).answer).toBe("not_answered");
    expect(pendingQuarantine(state)).toEqual([]);
    const call = state.turns.get("t1")?.tools.get("c1");
    if (call === undefined) {
      throw new Error("no call");
    }
    expect(toolStatus(call, true, 0)).toEqual({
      kind: "quarantined",
      word: "not answered · left in quarantine",
    });
  });
});
