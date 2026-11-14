// The exits of auto (AU-Auto, errata 1, 6, 7 and 9): the reducer puts each exit event on
// its tool call, and the words of the card are the words of efr's terminal.

import { describe, expect, it } from "vitest";

import type { Event, EventOf, SubscribeItem } from "@/lib/protocol/wire";
import type { ExitCallView, StreamState } from "@/lib/stream/reducer";
import { eventItem, run, toolCallStarted, turnStarted } from "@/lib/stream/testing";
import { exitNote, grantWords, pausedLine, reviewerRow, stoppedByPause } from "./exit-words";
import { toolStatus } from "./tool-block";

let seq = 0;
const event = (body: Event): SubscribeItem => eventItem(++seq, body);

function requested(
  call: string,
  line: string,
  extra: Partial<EventOf<"exit_requested">> = {},
): EventOf<"exit_requested"> {
  return {
    kind: "exit_requested",
    turn_id: "t1",
    call_id: call,
    kinds: ["host"],
    grants: [{ kind: "host", host: "github.com", port: 443 }],
    source: "predicted",
    record: {
      version: 1,
      user_messages: [],
      action: {
        tool: "shell",
        line,
        cwd: "/home/me/code/efr",
        scope: { kind: "project", value: "019a9b1c-3d00-7a10-8b20-000000000008" },
        exits: ["host"],
        source: "predicted",
      },
      facts: {
        hosts: [
          {
            host: "github.com",
            on_allow_list: false,
            named_in_user_messages: true,
            refused_by_proxy: false,
          },
        ],
      },
    },
    ...extra,
  };
}

function judged(call: string, extra: Partial<EventOf<"exit_judged">>): EventOf<"exit_judged"> {
  return {
    kind: "exit_judged",
    turn_id: "t1",
    call_id: call,
    judge: "classifier",
    verdict: "allow",
    ...extra,
  };
}

function stateOf(...events: Event[]): StreamState {
  return run([event(turnStarted("t1")), ...events.map(event)]);
}

function exitOf(state: StreamState, call: string): ExitCallView {
  const exit = state.turns.get("t1")?.tools.get(call)?.exit;
  if (exit === undefined) {
    throw new Error(`no exit on ${call}`);
  }
  return exit;
}

describe("the exit of a call in the reducer", () => {
  it("keeps the request, the review, the verdict and the rule on the tool call", () => {
    const started = stateOf(
      toolCallStarted("t1", "c1", "shell", { command: "git push" }),
      requested("c1", "git push -u origin fix/steer-race"),
      { kind: "exit_review_started", turn_id: "t1", call_id: "c1", kinds: ["host"] },
    );
    expect(exitOf(started, "c1")).toMatchObject({
      kinds: ["host"],
      line: "git push -u origin fix/steer-race",
      hosts: ["github.com"],
      reviewing: true,
      judged: undefined,
    });
    const done = run(
      [
        event(judged("c1", { risk: "low", rationale: "you asked to push the fix" })),
        event({
          kind: "always_rule_saved",
          turn_id: "t1",
          call_id: "c1",
          outcome: { kind: "saved", index: 2, rule: "host github.com:443" },
        }),
      ],
      started,
    );
    expect(exitOf(done, "c1")).toMatchObject({
      reviewing: false,
      judged: { judge: "classifier", verdict: "allow", risk: "low" },
      saved: { kind: "saved", index: 2 },
    });
  });

  it("keeps an exit that comes before its call starts", () => {
    const state = stateOf(
      requested("c1", "curl -sI https://example.com"),
      toolCallStarted("t1", "c1", "shell", { command: "curl -sI https://example.com" }),
    );
    expect(exitOf(state, "c1").line).toBe("curl -sI https://example.com");
  });

  it("makes the pause of the reviewer a part of its turn, once", () => {
    const pause: Event = {
      kind: "classifier_paused",
      turn_id: "t1",
      reason: "in_a_row",
      denials: 3,
    };
    const state = stateOf(toolCallStarted("t1", "c1", "shell"), pause, pause);
    const turn = state.turns.get("t1");
    expect(turn?.pause).toEqual({ reason: "in_a_row", denials: 3 });
    expect(turn?.parts.filter((part) => part.kind === "pause")).toHaveLength(1);
  });
});

describe("the words of an exit", () => {
  it("names each grant as the terminal does", () => {
    expect(grantWords({ kind: "write", path: "/home/me/.cache/pnpm" })).toBe(
      "~/.cache/pnpm writable",
    );
    expect(grantWords({ kind: "host", host: "github.com", port: 443 })).toBe(
      "github.com:443 reachable",
    );
    expect(grantWords({ kind: "unmask", path: "/home/me/.config/gh/hosts.yml" })).toBe(
      "~/.config/gh/hosts.yml readable",
    );
    expect(grantWords({ kind: "socket", path: "/home/me/.ssh/agent.sock" })).toBe(
      "the socket ~/.ssh/agent.sock",
    );
    expect(grantWords({ kind: "env", name: "GH_TOKEN" })).toBe("the variable GH_TOKEN");
    expect(grantWords({ kind: "keyring" })).toBe("the keyring (every unlocked entry)");
    expect(grantWords({ kind: "open_network" })).toBe("full network");
    expect(grantWords({ kind: "desktop", desktop: "hyprland" })).toBe("the Hyprland socket");
  });

  it("puts the grants on the line of the reviewer's allow, after a colon", () => {
    const state = stateOf(
      toolCallStarted("t1", "c1", "shell"),
      requested("c1", "git push", {
        grants: [
          { kind: "host", host: "github.com", port: 443 },
          { kind: "env", name: "GH_TOKEN" },
        ],
      }),
      judged("c1", { risk: "low", rationale: "you asked to push the fix" }),
    );
    expect(exitNote(exitOf(state, "c1"), "git push", undefined)).toEqual({
      text: "reviewer allowed: network (github.com) for this call (low risk; you asked to push the fix): github.com:443 reachable, the variable GH_TOKEN",
      tone: "text_subtle",
      loud: false,
    });
  });

  it("says what the call asks for while the reviewer judges it", () => {
    const state = stateOf(toolCallStarted("t1", "c1", "shell"), requested("c1", "gh pr create"), {
      kind: "exit_review_started",
      turn_id: "t1",
      call_id: "c1",
      kinds: ["host"],
    });
    const exit = exitOf(state, "c1");
    expect(exitNote(exit, "gh pr create", undefined)?.text).toBe("github.com:443 reachable");
    const call = state.turns.get("t1")?.tools.get("c1");
    expect(call === undefined ? undefined : toolStatus(call, false, 0)).toEqual({
      kind: "reviewing",
    });
  });

  it("names a refusal with the line and the reason, and the call did not run", () => {
    const state = stateOf(
      toolCallStarted("t1", "c1", "shell"),
      requested("c1", "curl -sI https://example.com"),
      judged("c1", {
        verdict: "deny",
        risk: "medium",
        rationale: "no message of yours names example.com",
      }),
    );
    expect(exitNote(exitOf(state, "c1"), "", undefined)).toEqual({
      text: "reviewer refused: curl -sI https://example.com: no message of yours names example.com",
      tone: "text_muted",
      loud: false,
    });
    const call = state.turns.get("t1")?.tools.get("c1");
    expect(call === undefined ? undefined : toolStatus(call, false, 0)).toEqual({
      kind: "closed",
      word: "refused",
    });
  });

  it("leads a refusal of efr's own floor with efr:, with its text only when efrd sends one", () => {
    const bare = stateOf(
      toolCallStarted("t1", "c1", "shell"),
      requested("c1", "cp ~/notes/hosts ~/.ssh/config"),
      judged("c1", { judge: "floor", verdict: "deny", category: "secret" }),
    );
    expect(exitNote(exitOf(bare, "c1"), "", undefined)?.text).toBe(
      "efr: cp ~/notes/hosts ~/.ssh/config",
    );
    const said = stateOf(
      toolCallStarted("t1", "c2", "shell"),
      requested("c2", "rm -rf ~/x"),
      judged("c2", { judge: "floor", verdict: "deny", rationale: "only you give up ~/x" }),
    );
    expect(exitNote(exitOf(said, "c2"), "", undefined)?.text).toBe(
      "efr: rm -rf ~/x: only you give up ~/x",
    );
  });

  it("says that no reviewer answered, and that nothing ran", () => {
    const state = stateOf(
      toolCallStarted("t1", "c1", "shell"),
      requested("c1", "git push"),
      judged("c1", { verdict: "deny", skipped: ["gpt-6-luna: timeout"], chain_step: "user" }),
    );
    expect(exitNote(exitOf(state, "c1"), "", undefined)?.text).toBe(
      "reviewer: no answer (skipped gpt-6-luna: timeout); nothing ran",
    );
    const call = state.turns.get("t1")?.tools.get("c1");
    expect(call === undefined ? undefined : toolStatus(call, false, 0)).toEqual({
      kind: "closed",
      word: "did not run",
    });
  });

  it("names the always rule that covered a call, with what the call got", () => {
    const state = stateOf(
      toolCallStarted("t1", "c1", "shell"),
      requested("c1", "cargo update -p rusqlite", {
        grants: [{ kind: "host", host: "index.crates.io", port: 443 }],
      }),
      judged("c1", {
        judge: "always",
        category: "sandbox.allow[2]",
        rationale: "network index.crates.io for cargo",
      }),
    );
    expect(exitNote(exitOf(state, "c1"), "", undefined)?.text).toBe(
      "allowed by your rule sandbox.allow[2]: network index.crates.io for cargo: index.crates.io:443 reachable",
    );
  });

  it("marks the whole network as the loudest grant", () => {
    const state = stateOf(
      toolCallStarted("t1", "c1", "shell"),
      requested("c1", "ssh laptop uptime", { grants: [{ kind: "open_network" }] }),
      judged("c1", { risk: "medium", rationale: "you asked for the uptime" }),
    );
    expect(exitNote(exitOf(state, "c1"), "", undefined)?.loud).toBe(true);
  });

  it("says the pause asks nothing, and the next action stops the turn", () => {
    expect(pausedLine({ reason: "in_a_row", denials: 3 })).toBe(
      "reviewer paused for this turn: 3 actions in a row were refused; the next action that needs it stops the turn",
    );
    expect(pausedLine({ reason: "per_turn", denials: 10 })).toBe(
      "reviewer paused for this turn: 10 actions of this turn were refused; the next action that needs it stops the turn",
    );
    const pause = { reason: "in_a_row", denials: 3 } as const;
    const state = stateOf(
      toolCallStarted("t1", "c1", "shell"),
      requested("c1", "git push origin fix/steer-race:main"),
    );
    const call = state.turns.get("t1")?.tools.get("c1");
    if (call === undefined) {
      throw new Error("no call");
    }
    expect(stoppedByPause(call, pause, false, true)).toBe(true);
    expect(stoppedByPause(call, pause, false, false)).toBe(false);
    expect(stoppedByPause(call, pause, true, true)).toBe(false);
    expect(exitNote(exitOf(state, "c1"), "", pause)?.text).toBe(
      "auto stopped this turn: 3 actions in a row were refused",
    );
    expect(toolStatus(call, true, 0, undefined, undefined, true)).toEqual({
      kind: "closed",
      word: "did not run",
    });
  });

  it("says what the reviewer thought before it handed a question over", () => {
    expect(reviewerRow("ask_user", "no message of yours names this host")).toBe(
      "reviewer: unsure; no message of yours names this host",
    );
    expect(reviewerRow("deny", "")).toBe("reviewer: it would refuse this");
  });
});
