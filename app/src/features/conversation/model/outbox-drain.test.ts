import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { backoffDelayMs } from "@/lib/net/backoff";
import { ConnectionError, ServerError } from "@/lib/net/errors";
import { FakeClock, flush } from "@/lib/net/testing";
import type { ErrorCode, PromptSend, PromptSendResult } from "@/lib/protocol/generated";
import { MAX_SEND_ATTEMPTS, resolveAlias, type Outbox, type OutboxTarget } from "./outbox";
import { OUTBOX_BACKOFF, blockedLanes, createOutboxDrain, type OutboxDrain } from "./outbox-drain";
import { MemoryOutboxStorage } from "./outbox-storage";
import {
  CONVERSATION_A,
  CONVERSATION_B,
  DESKTOP,
  FakeEfrd,
  LAPTOP,
  testOutbox,
} from "./outbox-testing";

const TO_A: OutboxTarget = { kind: "conversation", conversationId: CONVERSATION_A };
const TO_B: OutboxTarget = { kind: "conversation", conversationId: CONVERSATION_B };

/** A send that the test answers by hand, call by call. */
interface Call {
  readonly params: PromptSend;
  resolve(result: PromptSendResult): void;
  reject(error: unknown): void;
}

function manualSend(): {
  readonly calls: Call[];
  readonly send: (params: PromptSend) => Promise<PromptSendResult>;
} {
  const calls: Call[] = [];
  const send = (params: PromptSend): Promise<PromptSendResult> =>
    new Promise((resolve, reject) => {
      calls.push({ params, resolve, reject });
    });
  return { calls, send };
}

function serverError(code: ErrorCode, data?: Record<string, unknown>): ServerError {
  if (data === undefined) {
    return new ServerError({ code, message: `efrd said ${code}` });
  }
  return new ServerError({ code, message: `efrd said ${code}`, data });
}

function drainOf(
  outbox: Outbox,
  send: (params: PromptSend) => Promise<PromptSendResult>,
  clock: FakeClock,
  machine = DESKTOP,
): OutboxDrain {
  return createOutboxDrain({ outbox, machine, send, clock, random: () => 0 });
}

function call(calls: readonly Call[], index: number): Call {
  const found = calls[index];
  if (found === undefined) {
    throw new Error(`no call ${index}`);
  }
  return found;
}

function states(outbox: Outbox): string[] {
  return outbox.store.get().entries.map((entry) => `${entry.text}:${entry.state}`);
}

describe("the outbox drain", () => {
  it("sends nothing before start and after stop", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const { calls, send } = manualSend();
    const drain = drainOf(outbox, send, clock);
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    await flush();
    expect(calls).toHaveLength(0);
    drain.start();
    expect(calls).toHaveLength(1);
    drain.stop();
    expect(drain.running).toBe(false);
    outbox.enqueue({ machine: DESKTOP, target: TO_B, text: "b" });
    await flush();
    expect(calls).toHaveLength(1);
    // A send in flight still ends after stop.
    call(calls, 0).resolve({
      conversation_id: CONVERSATION_A,
      queued: false,
      seq: 1,
      turn_id: "t",
    });
    await flush();
    expect(states(outbox)).toEqual(["a:sent", "b:pending"]);
  });

  it("sends the lanes of its machine in parallel and each lane in order", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const efrd = new FakeEfrd(CONVERSATION_A, CONVERSATION_B);
    const { calls, send } = manualSend();
    const drain = drainOf(outbox, send, clock);
    drain.start();
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a1" });
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a2" });
    outbox.enqueue({ machine: DESKTOP, target: TO_B, text: "b1" });
    outbox.enqueue({ machine: LAPTOP, target: TO_A, text: "laptop" });
    expect(calls.map((sent) => sent.params.text)).toEqual(["a1", "b1"]);
    const first = call(calls, 0);
    first.resolve(efrd.take(first.params));
    await flush();
    expect(calls.map((sent) => sent.params.text)).toEqual(["a1", "b1", "a2"]);
  });

  it("sends the prompts of a new conversation to the conversation that the first made", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const efrd = new FakeEfrd();
    const sent: PromptSendResult[] = [];
    const drain = createOutboxDrain({
      outbox,
      machine: DESKTOP,
      send: (params) => Promise.resolve(efrd.take(params)),
      clock,
      random: () => 0,
      onSent: (_entry, result) => sent.push(result),
    });
    const target = outbox.newConversation();
    if (target.kind !== "new") {
      throw new Error("a new target");
    }
    drain.start();
    outbox.enqueue({ machine: DESKTOP, target, text: "first" });
    outbox.enqueue({ machine: DESKTOP, target, text: "second" });
    await flush();
    expect(efrd.calls.map((params) => params.new_conversation === true)).toEqual([true, false]);
    const made = resolveAlias(outbox.store.get(), DESKTOP, target.localId);
    expect(made).toBeDefined();
    expect(efrd.taken.map((prompt) => prompt.conversationId)).toEqual([made, made]);
    expect(sent).toHaveLength(2);
  });

  it("resends with the same command id after a lost answer, and efrd takes it once", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const efrd = new FakeEfrd(CONVERSATION_A);
    let lose = true;
    const drain = drainOf(
      outbox,
      (params) => {
        const result = efrd.take(params);
        if (lose) {
          // efrd took the prompt, but the socket closed before its answer came.
          lose = false;
          return Promise.reject(new ConnectionError("socket", "the socket closed"));
        }
        return Promise.resolve(result);
      },
      clock,
    );
    const entry = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "once" });
    drain.start();
    await flush();
    // The lost connection holds the drain until the next ready.
    expect(drain.running).toBe(false);
    expect(states(outbox)).toEqual(["once:pending"]);
    expect(outbox.store.get().entries[0]).toMatchObject({
      attempts: 0,
      error: { code: "connection" },
    });
    await clock.advance(60_000);
    expect(efrd.calls).toHaveLength(1);

    drain.start();
    await flush();
    expect(efrd.calls.map((params) => params.command_id)).toEqual([
      entry.commandId,
      entry.commandId,
    ]);
    expect(efrd.taken).toHaveLength(1);
    expect(states(outbox)).toEqual(["once:sent"]);
  });

  it("sends again on the new connection when the old socket closes late", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const { calls, send } = manualSend();
    const drain = drainOf(outbox, send, clock);
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    drain.start();
    expect(calls).toHaveLength(1);
    // The app came back to the foreground: the client opened a new socket and was
    // ready before the old socket closed.
    drain.stop();
    drain.start();
    call(calls, 0).reject(new ConnectionError("socket", "the old socket closed"));
    await flush();
    expect(drain.running).toBe(true);
    expect(calls).toHaveLength(2);
    expect(call(calls, 1).params.command_id).toBe(call(calls, 0).params.command_id);
  });

  it("waits the time that a busy error asks for", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const { calls, send } = manualSend();
    const drain = drainOf(outbox, send, clock);
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    drain.start();
    call(calls, 0).reject(serverError("busy", { retry_after_ms: 1_500 }));
    await flush();
    expect(states(outbox)).toEqual(["a:pending"]);
    await clock.advance(1_499);
    expect(calls).toHaveLength(1);
    await clock.advance(1);
    expect(calls).toHaveLength(2);
    expect(call(calls, 1).params.command_id).toBe(call(calls, 0).params.command_id);
  });

  it("waits on the backoff ladder after an internal error and fails at the limit", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const { calls, send } = manualSend();
    const drain = drainOf(outbox, send, clock);
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a2" });
    drain.start();
    for (let attempt = 0; attempt < MAX_SEND_ATTEMPTS - 1; attempt++) {
      expect(calls).toHaveLength(attempt + 1);
      call(calls, attempt).reject(serverError("internal"));
      await flush();
      const wait = backoffDelayMs(attempt, 0, OUTBOX_BACKOFF);
      await clock.advance(wait - 1);
      expect(calls).toHaveLength(attempt + 1);
      await clock.advance(1);
    }
    expect(calls).toHaveLength(MAX_SEND_ATTEMPTS);
    call(calls, MAX_SEND_ATTEMPTS - 1).reject(serverError("internal"));
    await flush();
    // The failed entry no longer holds its lane.
    expect(states(outbox)).toEqual(["a:failed", "a2:sending"]);
    expect(call(calls, MAX_SEND_ATTEMPTS).params.text).toBe("a2");
  });

  it("fails an entry that efrd refuses and goes on with the next", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const { calls, send } = manualSend();
    const drain = drainOf(outbox, send, clock);
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a2" });
    drain.start();
    call(calls, 0).reject(serverError("not_found"));
    await flush();
    expect(states(outbox)).toEqual(["a:failed", "a2:sending"]);
    expect(outbox.store.get().entries[0]?.error).toEqual({
      code: "not_found",
      message: "efrd said not_found",
    });
  });

  it("fails an entry whose result is malformed while the connection stays ready", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const { calls, send } = manualSend();
    const drain = drainOf(outbox, send, clock);
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a2" });
    drain.start();
    call(calls, 0).reject(new ConnectionError("malformed", "the result of prompt.send is bad"));
    await flush();
    // The drain still runs, and the next entry of the lane goes out.
    expect(drain.running).toBe(true);
    expect(states(outbox)).toEqual(["a:failed", "a2:sending"]);
    expect(outbox.store.get().entries[0]?.error).toEqual({
      code: "connection",
      message: "the result of prompt.send is bad",
    });
    expect(call(calls, 1).params.text).toBe("a2");
  });

  it("takes an error that send throws, and an error that is not efrd's", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    let calls = 0;
    const drain = drainOf(
      outbox,
      () => {
        calls += 1;
        if (calls === 1) {
          throw new Error("a bug");
        }
        return Promise.reject(new Error("a result that the schema refused"));
      },
      clock,
    );
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    drain.start();
    await flush();
    expect(outbox.store.get().entries[0]).toMatchObject({
      state: "pending",
      attempts: 1,
      error: { code: "client", message: "a bug" },
    });
    await clock.advance(backoffDelayMs(0, 0, OUTBOX_BACKOFF));
    expect(calls).toBe(2);
    expect(outbox.store.get().entries[0]).toMatchObject({ attempts: 2 });
  });

  it("sends the entries of the last launch on the first ready", async () => {
    const clock = new FakeClock();
    const storage = new MemoryOutboxStorage();
    const before = testOutbox(storage).outbox;
    const entry = before.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    before.begin(entry.commandId);
    await before.flush();

    const { outbox } = testOutbox(storage, "11111111");
    await outbox.hydrate();
    const efrd = new FakeEfrd(CONVERSATION_A);
    const drain = drainOf(outbox, (params) => Promise.resolve(efrd.take(params)), clock);
    drain.start();
    await flush();
    expect(efrd.taken.map((prompt) => prompt.commandId)).toEqual([entry.commandId]);
  });

  it("keeps the order of each lane whatever the network does", async () => {
    // Each step adds a prompt, or answers the oldest call with a result, a lost
    // connection or an error.
    const step = fc.oneof(
      fc.record({ kind: fc.constant("add" as const), lane: fc.integer({ min: 0, max: 2 }) }),
      fc.record({
        kind: fc.constant("answer" as const),
        outcome: fc.constantFrom("ok", "lost", "lost-after", "busy", "invalid"),
      }),
    );
    await fc.assert(
      fc.asyncProperty(fc.array(step, { maxLength: 40 }), async (steps) => {
        const clock = new FakeClock();
        const { outbox } = testOutbox();
        const efrd = new FakeEfrd(CONVERSATION_A, CONVERSATION_B);
        const { calls, send } = manualSend();
        const drain = drainOf(outbox, send, clock);
        const fresh = outbox.newConversation();
        const targets = [TO_A, TO_B, fresh];
        const added: { readonly lane: number; readonly text: string }[] = [];
        let answered = 0;
        const conversationOf = (lane: number): string | undefined => {
          const target = targets[lane];
          if (target === undefined) {
            return undefined;
          }
          if (target.kind === "conversation") {
            return target.conversationId;
          }
          return resolveAlias(outbox.store.get(), DESKTOP, target.localId);
        };
        drain.start();
        for (const next of steps) {
          if (next.kind === "add") {
            const text = `p${added.length}`;
            added.push({ lane: next.lane, text });
            outbox.enqueue({ machine: DESKTOP, target: targets[next.lane] ?? TO_A, text });
          } else if (answered < calls.length) {
            const pending = call(calls, answered);
            answered += 1;
            if (next.outcome === "ok" || efrd.has(pending.params.command_id)) {
              // efrd answers a command id that it took with its receipt, never an error.
              pending.resolve(efrd.take(pending.params));
            } else if (next.outcome === "lost-after") {
              efrd.take(pending.params);
              pending.reject(new ConnectionError("socket", "closed"));
            } else if (next.outcome === "lost") {
              pending.reject(new ConnectionError("socket", "closed"));
            } else if (next.outcome === "busy") {
              pending.reject(serverError("busy", { retry_after_ms: 10 }));
            } else {
              pending.reject(serverError("invalid"));
            }
            await flush();
            if (!drain.running) {
              drain.start();
            }
          }
          await clock.advance(10);
        }
        // The network heals: every call gets its result.
        for (let rounds = 0; rounds < 200 && answered < calls.length; rounds++) {
          const pending = call(calls, answered);
          answered += 1;
          pending.resolve(efrd.take(pending.params));
          await flush();
          await clock.advance(10);
        }
        const failed = new Set(
          outbox.store
            .get()
            .entries.filter((entry) => entry.state === "failed")
            .map((entry) => entry.text),
        );
        // Every prompt reached efrd once, or failed, and each lane kept its order.
        for (let lane = 0; lane < targets.length; lane++) {
          const expected = added
            .filter((prompt) => prompt.lane === lane && !failed.has(prompt.text))
            .map((prompt) => prompt.text);
          const conversation = conversationOf(lane);
          const got = efrd.taken
            .filter((prompt) => prompt.conversationId === conversation)
            .map((prompt) => prompt.text);
          expect(got).toEqual(expected);
        }
        expect(new Set(efrd.taken.map((prompt) => prompt.commandId)).size).toBe(efrd.taken.length);
      }),
      { seed: 20261026, numRuns: 150 },
    );
  });
});

describe("the blocked lanes of a machine", () => {
  it("names a head in flight with prompts behind it, and no other lane", () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const { send } = manualSend();
    const drain = drainOf(outbox, send, clock);
    const head = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    outbox.enqueue({ machine: DESKTOP, target: TO_B, text: "b" });
    expect(blockedLanes(outbox.store.get(), DESKTOP)).toEqual([]);
    drain.start();
    // Both heads are in flight, and nothing waits behind them yet.
    expect(blockedLanes(outbox.store.get(), DESKTOP)).toEqual([]);
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a2" });
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a3" });
    expect(blockedLanes(outbox.store.get(), DESKTOP)).toEqual([
      { head: head.commandId, waiting: 2 },
    ]);
    expect(blockedLanes(outbox.store.get(), LAPTOP)).toEqual([]);
  });
});
