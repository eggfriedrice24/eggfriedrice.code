import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { backoffDelayMs } from "@/lib/net/backoff";
import { ConnectionError, ServerError } from "@/lib/net/errors";
import { FakeClock, flush } from "@/lib/net/testing";
import type { ErrorCode, PromptSend, PromptSendResult } from "@/lib/protocol/generated";
import { FILE_GONE } from "../../../../modules/efr-files/src/EfrFiles.types";
import { UPLOAD_LOST, fileGoneLine } from "./attachments-format";
import {
  MAX_REUPLOADS,
  MAX_SEND_ATTEMPTS,
  resolveAlias,
  type Outbox,
  type OutboxTarget,
} from "./outbox";
import {
  OUTBOX_BACKOFF,
  blockedLanes,
  createOutboxDrain,
  type OutboxDrain,
  type OutboxUploads,
} from "./outbox-drain";
import { MemoryOutboxStorage } from "./outbox-storage";
import {
  CONVERSATION_A,
  CONVERSATION_B,
  DESKTOP,
  FILE_A,
  FILE_B,
  FakeEfrd,
  LAPTOP,
  fakeUploads,
  outboxFile,
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
  return createOutboxDrain({
    outbox,
    machine,
    send,
    clock,
    random: () => 0,
    autoAllowed: () => true,
  });
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
      autoAllowed: () => true,
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

/**
 * A drain whose prompts go to `efrd`, with `uploads` for their files. `before` runs
 * before each `prompt.send`, as a restart of efrd that loses uploads.
 */
function uploadDrain(
  outbox: Outbox,
  efrd: FakeEfrd,
  uploads: OutboxUploads,
  clock: FakeClock,
  before: () => void = () => undefined,
): OutboxDrain {
  return createOutboxDrain({
    outbox,
    machine: DESKTOP,
    send: (params) => {
      before();
      try {
        return Promise.resolve(efrd.take(params));
      } catch (error) {
        return Promise.reject(error);
      }
    },
    clock,
    random: () => 0,
    autoAllowed: () => true,
    uploads,
  });
}

/** The uploads of `efrd` whose puts wait until the test answers each one. */
function heldUploads(efrd: FakeEfrd): {
  readonly uploads: ReturnType<typeof fakeUploads>;
  readonly answers: (() => void)[];
} {
  const answers: (() => void)[] = [];
  const base = fakeUploads(efrd);
  const uploads = {
    ...base,
    put: (params: Parameters<OutboxUploads["put"]>[0]) =>
      new Promise<ReturnType<FakeEfrd["uploads"]["put"]>>((resolve) => {
        answers.push(() => resolve(efrd.uploads.put(params)));
      }),
  };
  return { uploads, answers };
}

/** Lets the drain run its rounds of uploads and sends: one round needs many microtasks. */
async function drainRounds(): Promise<void> {
  for (let round = 0; round <= MAX_REUPLOADS; round++) {
    await flush();
  }
}

/** Each put as `upload@offset`, with the last letter of the upload id. */
function puts(efrd: FakeEfrd): string[] {
  return efrd.uploads.puts.map((params) => `${params.upload_id.slice(-1)}@${params.offset}`);
}

describe("the uploads of a prompt", () => {
  it("uploads each file in chunks, one chunk at a time, then sends the prompt that names them", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const efrd = new FakeEfrd(CONVERSATION_A);
    const { uploads, answers } = heldUploads(efrd);
    const drain = uploadDrain(outbox, efrd, uploads, clock);
    const entry = outbox.enqueue({
      machine: DESKTOP,
      target: TO_A,
      text: "look",
      attachments: [outboxFile(FILE_A, 10), outboxFile(FILE_B, 3, "b.jpg")],
    });
    drain.start();
    await flush();
    for (let i = 0; i < 4; i++) {
      // One chunk in flight: the next waits for the answer of the one before.
      expect(answers).toHaveLength(1);
      answers.shift()?.();
      await flush();
    }
    expect(answers).toHaveLength(0);
    expect(puts(efrd)).toEqual(["a@0", "a@4", "a@8", "b@0"]);
    expect(efrd.calls).toEqual([
      {
        command_id: entry.commandId,
        conversation_id: CONVERSATION_A,
        text: "look",
        attachments: [FILE_A, FILE_B],
      },
    ]);
    expect(states(outbox)).toEqual(["look:sent"]);
  });

  it("goes on after a lost link from the bytes that efrd has, and keeps only the end on the disk", async () => {
    const clock = new FakeClock();
    const storage = new MemoryOutboxStorage();
    const { outbox } = testOutbox(storage);
    const efrd = new FakeEfrd(CONVERSATION_A);
    const base = fakeUploads(efrd);
    let count = 0;
    const drain = uploadDrain(
      outbox,
      efrd,
      {
        ...base,
        put: (params) => {
          count += 1;
          // efrd takes the second chunk, but the link goes before its answer.
          const answer = efrd.uploads.put(params);
          return count === 2
            ? Promise.reject(new ConnectionError("socket", "the socket closed"))
            : Promise.resolve(answer);
        },
      },
      clock,
    );
    outbox.enqueue({
      machine: DESKTOP,
      target: TO_A,
      text: "look",
      attachments: [outboxFile(FILE_A, 10)],
    });
    drain.start();
    await flush();
    expect(states(outbox)).toEqual(["look:pending"]);
    expect(outbox.store.get().entries[0]?.attachments?.[0]).toMatchObject({
      received: 4,
      uploaded: false,
    });
    await outbox.flush();
    expect((await storage.load())[0]?.attachments?.[0]?.received).toBe(0);

    drain.start();
    await flush();
    // The chunk at 4 goes again; efrd answers 8 and writes nothing, so the next is at 8.
    expect(puts(efrd)).toEqual(["a@0", "a@4", "a@4", "a@8"]);
    expect(efrd.taken).toHaveLength(1);
    await outbox.flush();
    expect((await storage.load())[0]?.attachments?.[0]).toMatchObject({
      received: 10,
      uploaded: true,
    });
  });

  it("uploads the files that efrd lost again, with the same ids and the same command id", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const efrd = new FakeEfrd(CONVERSATION_A);
    let restarted = false;
    const drain = uploadDrain(outbox, efrd, fakeUploads(efrd), clock, () => {
      if (!restarted) {
        restarted = true;
        efrd.uploads.discard(FILE_B);
      }
    });
    const entry = outbox.enqueue({
      machine: DESKTOP,
      target: TO_A,
      text: "look",
      attachments: [outboxFile(FILE_A, 4), outboxFile(FILE_B, 4, "b.jpg")],
    });
    drain.start();
    await drainRounds();
    expect(puts(efrd)).toEqual(["a@0", "b@0", "b@0"]);
    expect(efrd.calls.map((params) => params.command_id)).toEqual([
      entry.commandId,
      entry.commandId,
    ]);
    expect(efrd.taken).toHaveLength(1);
    expect(outbox.store.get().entries[0]).toMatchObject({ state: "sent", reuploads: 1 });
  });

  it("fails the prompt when efrd lost its uploads more than MAX_REUPLOADS times", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const efrd = new FakeEfrd(CONVERSATION_A);
    const drain = uploadDrain(outbox, efrd, fakeUploads(efrd), clock, () => {
      efrd.uploads.discard(FILE_A);
    });
    outbox.enqueue({
      machine: DESKTOP,
      target: TO_A,
      text: "look",
      attachments: [outboxFile(FILE_A, 4)],
    });
    drain.start();
    await drainRounds();
    expect(efrd.calls).toHaveLength(MAX_REUPLOADS + 1);
    expect(efrd.taken).toHaveLength(0);
    expect(outbox.store.get().entries[0]).toMatchObject({
      state: "failed",
      reuploads: MAX_REUPLOADS,
      error: { code: "upload", message: UPLOAD_LOST },
    });
  });

  it("fails the prompt at once and names the file when iOS purged its copy", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const efrd = new FakeEfrd(CONVERSATION_A);
    const gone = Object.assign(new Error("The file is not there any more"), { code: FILE_GONE });
    const uploads = { ...fakeUploads(efrd), readChunk: () => Promise.reject(gone) };
    const drain = uploadDrain(outbox, efrd, uploads, clock);
    outbox.enqueue({
      machine: DESKTOP,
      target: TO_A,
      text: "look",
      attachments: [outboxFile(FILE_A, 4)],
    });
    drain.start();
    await drainRounds();
    expect(efrd.uploads.puts).toEqual([]);
    expect(efrd.calls).toHaveLength(0);
    expect(outbox.store.get().entries[0]).toMatchObject({
      state: "failed",
      error: { code: "upload", message: fileGoneLine("shot.jpg") },
    });
  });

  it("stops the upload of a prompt that the user withdraws, and discards the chunk in flight", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const efrd = new FakeEfrd(CONVERSATION_A);
    const { uploads, answers } = heldUploads(efrd);
    const drain = uploadDrain(outbox, efrd, uploads, clock);
    const entry = outbox.enqueue({
      machine: DESKTOP,
      target: TO_A,
      text: "look",
      attachments: [outboxFile(FILE_A, 10)],
    });
    drain.start();
    await flush();
    expect(outbox.discard(entry.commandId)?.commandId).toBe(entry.commandId);
    answers.shift()?.();
    await flush();
    expect(answers).toHaveLength(0);
    expect(uploads.discards).toEqual([FILE_A]);
    expect(efrd.uploads.received(FILE_A)).toBeUndefined();
    expect(efrd.calls).toHaveLength(0);
    expect(outbox.store.get().entries).toEqual([]);
  });

  it("keeps a prompt whose files are all up, because its send is in flight", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const efrd = new FakeEfrd(CONVERSATION_A);
    const { calls, send } = manualSend();
    const drain = createOutboxDrain({
      outbox,
      machine: DESKTOP,
      send,
      clock,
      random: () => 0,
      autoAllowed: () => true,
      uploads: fakeUploads(efrd),
    });
    const entry = outbox.enqueue({
      machine: DESKTOP,
      target: TO_A,
      text: "look",
      attachments: [outboxFile(FILE_A, 2)],
    });
    drain.start();
    await flush();
    expect(calls).toHaveLength(1);
    expect(outbox.discard(entry.commandId)).toBeUndefined();
    expect(states(outbox)).toEqual(["look:sending"]);
  });

  it("fails a prompt with files for an efrd that takes none, and sends no chunk", async () => {
    const clock = new FakeClock();
    const { outbox } = testOutbox();
    const efrd = new FakeEfrd(CONVERSATION_A);
    const uploads = { ...fakeUploads(efrd), chunkBytes: () => undefined };
    const drain = uploadDrain(outbox, efrd, uploads, clock);
    outbox.enqueue({
      machine: DESKTOP,
      target: TO_A,
      text: "look",
      attachments: [outboxFile(FILE_A, 2)],
    });
    drain.start();
    await flush();
    expect(efrd.uploads.puts).toEqual([]);
    expect(efrd.calls).toEqual([]);
    expect(outbox.store.get().entries[0]).toMatchObject({
      state: "failed",
      error: { code: "upload" },
    });
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
