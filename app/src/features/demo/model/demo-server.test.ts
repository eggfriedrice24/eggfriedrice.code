import { describe, expect, it } from "vitest";

import { FakeClock, flush } from "@/lib/net/testing";
import type { SocketListener } from "@/lib/net/socket";
import { decodeSubscribeItem } from "@/lib/protocol/envelope";
import type { MachineId } from "@/features/machines/model/machine-ref";
import { approvalCases, demoConversation, type DemoConversationKind } from "./demo-conversations";
import { sendAll, type WireItem } from "./demo-log";
import { demoMachine, demoPairingCode, demoUrl, type DemoMachineKey } from "./demo-machines";
import { DemoServer, type DemoStage } from "./demo-server";
import { DEMO_DEVICE_NAME, sandboxServices, withDemoMachines } from "./demo-services";
import { STAGES } from "./demo-stages";
import type { DemoStep } from "./demo-script";

const NOW = Date.parse("2026-10-26T12:00:00Z");

const KINDS: readonly DemoConversationKind[] = [
  "streaming",
  "approval",
  "waiting",
  "notes",
  "disk",
  "rename",
  "failed",
  "long",
  "markdown",
  "tools",
  "messages",
  "diff",
  "fast",
];

/** Every step of `steps`, with both answers of each hold. */
function everyStep(steps: readonly DemoStep[]): DemoStep[] {
  return steps.flatMap((step) =>
    step.kind === "hold" ? [...everyStep(step.allow), ...everyStep(step.deny)] : [step],
  );
}

/** Expects that `item` decodes as a kind that the app knows and reads. */
function expectKnown(item: WireItem): void {
  const decoded = decodeSubscribeItem(JSON.parse(JSON.stringify(item)));
  expect(decoded.ok).toBe(true);
  const value = decoded.ok ? decoded.value : undefined;
  expect(value?.kind).not.toBe("other");
  const inner =
    value?.kind === "event" ? value.event.kind : value?.kind === "draft" ? value.draft.kind : "";
  expect(inner).not.toBe("other");
}

function ids(): () => string {
  let n = 0;
  return () => {
    n += 1;
    return `0199d3e0-de32-7000-8000-${n.toString(16).padStart(12, "0")}`;
  };
}

/** A server, and sandbox services on it with `machines` paired. */
async function sandbox(machines: readonly DemoMachineKey[]) {
  const clock = new FakeClock(NOW);
  const server = new DemoServer({ clock, confirmMs: 1000 });
  const errors: unknown[] = [];
  const box = await sandboxServices({
    server,
    machines,
    schedule: (callback) => {
      callback();
      return () => undefined;
    },
    random: () => 0,
    onError: (error) => errors.push(error),
  });
  await clock.advance(10);
  return { clock, server, errors, ...box };
}

/** Moves `clock` in steps of 100 ms until `promise` settles, for at most `ms`. */
async function settleOn<T>(clock: FakeClock, promise: Promise<T>, ms = 10_000): Promise<T> {
  const state = { settled: false };
  const watched = promise.finally(() => {
    state.settled = true;
  });
  watched.catch(() => undefined);
  for (let waited = 0; waited < ms && !state.settled; waited += 100) {
    await clock.advance(100);
  }
  return watched;
}

function viewOf(services: Awaited<ReturnType<typeof sandbox>>["services"], machine: MachineId) {
  return services.runtime.machine(machine)?.connection.get().view;
}

describe("the scripts of the demo machines", () => {
  it("send only items that the app reads, in every conversation and every answer", () => {
    for (const kind of KINDS) {
      const script = demoConversation(kind, ids());
      const steps = everyStep([...script.history, ...script.live]);
      const { log, items } = sendAll(
        {
          id: "0199d3e0-de32-7000-8000-0000000000aa",
          title: script.title,
          cwd: "/",
          createdAt: NOW,
        },
        steps,
        NOW,
      );
      expect(items.length).toBeGreaterThan(0);
      for (const item of items) {
        expectKnown(item);
      }
      expectKnown(log.snapshot());
    }
  });

  it("have every state of the approval block, each in a stream of its own", () => {
    const cases = approvalCases(ids());
    expect(cases.map((one) => one.name)).toEqual([
      "waiting, with a reason",
      "waiting, without a reason",
      "long command",
      "file write with a diff",
      "sending the answer",
      "the answer failed",
      "answered: allowed here",
      "answered: denied in a terminal",
      "answer in the terminal: privilege",
      "answer in the terminal: background job",
      "answer in the terminal: always rule",
      "expired",
      "expired while you were away",
    ]);
    for (const one of cases) {
      const { items } = sendAll(
        { id: "0199d3e0-de32-7000-8000-0000000000ab", title: one.name, cwd: "/", createdAt: NOW },
        one.steps,
        NOW,
      );
      for (const item of items) {
        expectKnown(item);
      }
    }
  });
});

describe("the demo server", () => {
  it("pairs through the real pairing flow and connects the machine with its proof", async () => {
    const { clock, server, services, devices } = await sandbox([]);
    const desktop = demoMachine("desktop");
    const machine = await settleOn(
      clock,
      services.runtime.pair(demoPairingCode(desktop), { name: DEMO_DEVICE_NAME }),
    );
    expect(machine.id).toBe(desktop.daemonId);
    expect((await devices.loadAll()).map((record) => record.daemonId)).toEqual([desktop.daemonId]);
    await clock.advance(10);
    expect(viewOf(services, desktop.daemonId)?.kind).toBe("online");
    expect(server.openSockets("desktop")).toBe(1);
    services.dispose();
  });

  it("refuses a pairing code with a wrong token", async () => {
    const { clock, services } = await sandbox([]);
    const code = demoPairingCode(demoMachine("laptop")).replace(
      /t=.*$/,
      "t=d3JvbmctdG9rZW4tZm9yLWRlbW8",
    );
    const failure = settleOn(
      clock,
      services.runtime.pair(code, { name: DEMO_DEVICE_NAME }).catch((error: unknown) => error),
    );
    expect(await failure).toMatchObject({ failure: { kind: "token_refused" } });
    services.dispose();
  });

  it("lists the conversations of a stage, newest first, with their status", async () => {
    const { clock, server, services } = await sandbox(["desktop", "laptop"]);
    server.stage(STAGES.many);
    const desktop = demoMachine("desktop").daemonId;
    const recent = await services.runtime.call(desktop, "conversations.recent", { limit: 20 });
    await clock.advance(1);
    const titles = recent.conversations.map((one) => [one.summary.title, one.summary.status]);
    expect(titles[0]).toEqual(["keep the journal small", "awaiting_approval"]);
    expect(titles.slice(1, 4)).toEqual([
      ["update my notes and restart the web server", "idle"],
      ["why is the disk full", "idle"],
      ["rename the config keys to snake case", "idle"],
    ]);
    const updated = recent.conversations.map((one) => Date.parse(one.summary.updated_at));
    expect([...updated].sort((a, b) => b - a)).toEqual(updated);
    const search = await services.runtime.call(desktop, "conversations.search", {
      query: "Rename",
    });
    expect(search.rows.map((row) => row.conversation.summary.title)).toEqual([
      "rename the config keys to snake case",
    ]);
    services.dispose();
  });

  it("streams a conversation from its first subscription, as efrd does", async () => {
    const { clock, server, services } = await sandbox(["desktop"]);
    const refs = server.stage(STAGES.streaming);
    const ref = refs.get("streaming");
    expect(ref).toBeDefined();
    if (ref === undefined) {
      return;
    }
    const hold = services.runtime.hold(ref, "follow");
    await clock.advance(10);
    const before = hold?.stream.latest();
    expect(before?.loaded).toBe(true);
    expect(before?.order).toHaveLength(1);
    await clock.advance(60_000);
    const after = hold?.stream.latest();
    expect(after?.order).toHaveLength(3);
    const [, turn, queued] = (after?.order ?? []).map((id) => after?.turns.get(id));
    expect(turn?.prompt?.text).toBe("fix the flaky sum test");
    expect(turn?.end?.outcome).toBe("completed");
    expect(turn?.title).toBe("Reading the test");
    expect(turn?.tools.size).toBe(2);
    expect(queued?.prompt?.text).toBe("then run the whole suite");
    expect(queued?.end?.outcome).toBe("completed");
    hold?.release();
    services.dispose();
  });

  it("waits at a question until the phone answers, then plays the answer", async () => {
    const { clock, server, services } = await sandbox(["desktop"]);
    const ref = server.stage(STAGES.approvalHere).get("approval");
    if (ref === undefined) {
      throw new Error("the stage has no approval");
    }
    const hold = services.runtime.hold(ref, "follow");
    await clock.advance(30_000);
    const asking = hold?.stream.latest();
    const turnId = asking?.order.at(-1) ?? "";
    const approval = [...(asking?.turns.get(turnId)?.approvals.values() ?? [])][0];
    expect(approval?.status).toBe("pending");
    expect(approval?.reason).toBe(
      "Writes outside the project. cautious asks before a write outside the working directory.",
    );
    expect(server.conversation(ref.id)?.log.status()).toBe("awaiting_approval");
    const answer = services.runtime.call(ref.machine, "approval.respond", {
      conversation_id: ref.id,
      call_id: approval?.callId ?? "",
      decision: "allow",
      command_id: server.newId(),
    });
    await clock.advance(1);
    expect((await answer).seq).toBeGreaterThan(0);
    await clock.advance(30_000);
    const done = hold?.stream.latest()?.turns.get(turnId);
    expect([...(done?.approvals.values() ?? [])][0]?.status).toBe("allowed");
    expect(done?.end?.outcome).toBe("completed");
    const again = services.runtime.call(ref.machine, "approval.respond", {
      conversation_id: ref.id,
      call_id: approval?.callId ?? "",
      decision: "allow",
      command_id: server.newId(),
    });
    const refused = again.catch((error: unknown) => error);
    await clock.advance(1);
    expect(await refused).toMatchObject({ body: { code: "conflict" } });
    hold?.release();
    services.dispose();
  });

  it("starts a conversation for a prompt of the phone, once per command id", async () => {
    const { clock, server, services } = await sandbox(["desktop"]);
    server.stage(STAGES.machines);
    const desktop = demoMachine("desktop").daemonId;
    const params = { command_id: server.newId(), text: "hello demo", new_conversation: true };
    const first = services.runtime.call(desktop, "prompt.send", params);
    await clock.advance(1);
    const sent = await first;
    const again = services.runtime.call(desktop, "prompt.send", {
      ...params,
      conversation_id: sent.conversation_id,
      new_conversation: false,
    });
    await clock.advance(1);
    expect((await again).turn_id).toBe(sent.turn_id);
    const hold = services.runtime.hold({ machine: desktop, id: sent.conversation_id }, "follow");
    await clock.advance(30_000);
    const state = hold?.stream.latest();
    const turn = state?.turns.get(sent.turn_id);
    expect(turn?.prompt?.text).toBe("hello demo");
    expect(turn?.end?.outcome).toBe("completed");
    expect(turn?.messages.get(0)?.buffer.text).toContain("This is a demo machine");
    hold?.release();
    services.dispose();
  });

  it("steers, stops and takes back prompts of a running turn", async () => {
    const { clock, server, services } = await sandbox(["desktop"]);
    const ref = server.stage(STAGES.streaming).get("streaming");
    if (ref === undefined) {
      throw new Error("the stage has no streaming conversation");
    }
    const hold = services.runtime.hold(ref, "follow");
    await clock.advance(3000);
    const call = <M extends "turn.steer" | "turn.interrupt" | "prompt.withdraw">(
      method: M,
      params: Parameters<typeof services.runtime.call<M>>[2],
    ) => {
      const result = services.runtime.call(ref.machine, method, params);
      return clock.advance(1).then(() => result);
    };
    const steered = await call("turn.steer", {
      conversation_id: ref.id,
      command_id: server.newId(),
      text: "only this crate",
    });
    await clock.advance(1500);
    const queued = server.conversation(ref.id)?.prompt("one more", server.newId());
    await clock.advance(1);
    const withdrawn = await call("prompt.withdraw", {
      conversation_id: ref.id,
      command_id: server.newId(),
      target: { kind: "turn", turn_id: queued?.turnId ?? "" },
    });
    expect(withdrawn.withdrawn.text).toBe("one more");
    const stopped = await call("turn.interrupt", {
      conversation_id: ref.id,
      command_id: server.newId(),
    });
    expect(stopped.turn_id).toBe(steered.turn_id);
    await clock.advance(60_000);
    const state = hold?.stream.latest();
    const turn = state?.turns.get(steered.turn_id);
    expect(turn?.end?.outcome).toBe("interrupted");
    expect([...(turn?.steers.values() ?? [])][0]?.status).toBe("delivered");
    expect(state?.turns.get(queued?.turnId ?? "")?.end?.outcome).toBe("withdrawn");
    hold?.release();
    services.dispose();
  });

  it("changes the mode of a machine after the first subscription of the stage", async () => {
    const { clock, server, services } = await sandbox(["laptop"]);
    const ref = server.stage(STAGES.offline).get("laptop-disk");
    if (ref === undefined) {
      throw new Error("the stage has no laptop-disk");
    }
    const hold = services.runtime.hold(ref, "follow");
    await clock.advance(100);
    expect(hold?.stream.latest()?.loaded).toBe(true);
    expect(viewOf(services, ref.machine)?.kind).toBe("online");
    await clock.advance(2500);
    expect(server.mode("laptop")).toBe("offline");
    expect(viewOf(services, ref.machine)?.kind).toBe("unreachable");
    expect(hold?.stream.latest()?.order).toHaveLength(1);
    hold?.release();
    services.dispose();
  });

  it("gives each mode of a machine the connection state that the app shows", async () => {
    const cases: [NonNullable<DemoStage["modes"]>, string, string][] = [
      [{ desktop: "offline" }, "unreachable", "offline"],
      [{ desktop: "starting" }, "starting", "starting"],
      [{ desktop: "repair" }, "repair_needed", "repair"],
      [{ desktop: "old" }, "update_needed", "old"],
    ];
    for (const [modes, view] of cases) {
      const { clock, server, services } = await sandbox(["desktop"]);
      server.stage({ name: "mode", modes, conversations: [] });
      await clock.advance(60_000);
      expect(viewOf(services, demoMachine("desktop").daemonId)?.kind).toBe(view);
      services.dispose();
    }
  });

  it("keeps a silent machine connecting until the handshake times out", async () => {
    const { clock, server, services } = await sandbox(["homelab"]);
    server.stage({ name: "silent", modes: { homelab: "silent" }, conversations: [] });
    const id = demoMachine("homelab").daemonId;
    await clock.advance(1000);
    expect(viewOf(services, id)?.kind).toBe("connecting");
    await clock.advance(15_000);
    expect(viewOf(services, id)?.kind).toBe("unreachable");
    services.dispose();
  });

  it("delays the snapshot of a conversation that loads slowly", async () => {
    const { clock, server, services } = await sandbox(["desktop"]);
    const ref = server.stage(STAGES.loading).get("loading");
    if (ref === undefined) {
      throw new Error("the stage has no loading conversation");
    }
    const hold = services.runtime.hold(ref, "follow");
    await clock.advance(7000);
    expect(hold?.stream.latest()?.loaded).toBe(false);
    await clock.advance(1100);
    expect(hold?.stream.latest()?.loaded).toBe(true);
    hold?.release();
    services.dispose();
  });

  it("answers a second hello with conflict, as the liveness probe needs", async () => {
    const server = new DemoServer({ clock: new FakeClock(NOW) });
    const messages: unknown[] = [];
    const listener: SocketListener = {
      opened: () => undefined,
      message: (data) => messages.push(typeof data === "string" ? JSON.parse(data) : data),
      closed: () => undefined,
    };
    const socket = server.connect(demoUrl(demoMachine("desktop")), listener);
    await flush();
    socket.send(JSON.stringify({ id: 1, method: "hello", params: {} }));
    socket.send(JSON.stringify({ id: 2, method: "hello", params: {} }));
    await flush();
    expect(messages[0]).toMatchObject({ id: 1, item: { protocol: 1 } });
    expect(messages[1]).toEqual({ id: 1, end: true });
    expect(messages[2]).toMatchObject({ id: 2, error: { code: "conflict" } });
  });

  it("sends the demo addresses to the server and every other one to the network", async () => {
    const server = new DemoServer({ clock: new FakeClock(NOW) });
    const asked: string[] = [];
    const sockets = withDemoMachines(
      {
        sockets: (url) => {
          asked.push(url);
          return { send: () => undefined, close: () => undefined };
        },
        clock: server.clock,
        keys: {
          hardwareBacked: false,
          create: () => Promise.reject(new Error("no")),
          load: () => Promise.resolve(null),
          remove: () => Promise.resolve(),
        },
        devices: {
          loadAll: () => Promise.resolve([]),
          upsert: () => Promise.resolve(),
          remove: () => Promise.resolve(),
        },
        sha256: () => Promise.resolve(new Uint8Array(32)),
        newKeyAlias: () => "a",
        now: () => new Date(NOW),
        client: "test",
      },
      server,
    ).sockets;
    const quiet: SocketListener = {
      opened: () => undefined,
      message: () => undefined,
      closed: () => undefined,
    };
    sockets("ws://100.64.0.1:7419/", quiet);
    sockets(demoUrl(demoMachine("laptop")), quiet);
    await flush();
    expect(asked).toEqual(["ws://100.64.0.1:7419/"]);
    expect(server.openSockets("laptop")).toBe(1);
  });
});
