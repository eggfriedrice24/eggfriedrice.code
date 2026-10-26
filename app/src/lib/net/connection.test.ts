import { describe, expect, it } from "vitest";

import { base64Decode, hex } from "@/lib/protocol/bytes";
import { proveFields, proveMessage } from "@/lib/protocol/prove";
import { Connection, type ConnectionOptions, type StreamHandlers } from "./connection";
import { ConnectionError, ServerError } from "./errors";
import { DEFAULT_LIVENESS } from "./liveness";
import {
  DAEMON_ID,
  DEVICE_ID,
  FakeClock,
  FakeKey,
  FakeSockets,
  completeHandshake,
  flush,
  remoteHello,
} from "./testing";

const URL = "ws://100.64.0.1:7419/";

function setup(overrides: Partial<ConnectionOptions> = {}, pairing = false) {
  const clock = new FakeClock();
  const sockets = new FakeSockets();
  const key = new FakeKey();
  const closes: ConnectionError[] = [];
  const options: ConnectionOptions = {
    url: URL,
    sockets: sockets.factory,
    clock,
    client: "efr-app 0.0.0",
    daemonId: DAEMON_ID,
    device: { id: DEVICE_ID, key },
    onClose: (error) => closes.push(error),
    ...overrides,
  };
  const { device, ...withoutDevice } = options;
  const opening = Connection.open(pairing || device === undefined ? withoutDevice : options);
  // The test reads the outcome later; a rejection must not count as unhandled.
  opening.catch(() => {});
  return { clock, sockets, key, closes, opening };
}

function defined<T>(value: T | undefined): T {
  if (value === undefined) {
    throw new Error("the value is undefined");
  }
  return value;
}

async function failure(promise: Promise<unknown>): Promise<ConnectionError> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof ConnectionError) {
      return error;
    }
    throw error;
  }
  throw new Error("the promise did not reject");
}

async function ready() {
  const parts = setup();
  await completeHandshake(parts.sockets.last);
  const connection = await parts.opening;
  return { ...parts, connection, socket: parts.sockets.last };
}

function recorder() {
  const log: string[] = [];
  const handlers: StreamHandlers = {
    item: (item) => log.push(`item ${JSON.stringify(item)}`),
    end: () => log.push("end"),
    error: (body) => log.push(`error ${body.code}`),
    lost: (error) => log.push(`lost ${error.kind}`),
  };
  return { log, handlers };
}

describe("the handshake", () => {
  it("sends hello, signs the prove message, proves and is ready", async () => {
    const { sockets, key, opening } = setup();
    const socket = sockets.last;
    expect(socket.url).toBe(URL);
    expect(socket.sent).toEqual([]);
    socket.open();
    await flush();
    expect(socket.sent).toEqual([
      {
        id: 1,
        method: "hello",
        params: { protocol: 1, origin: "phone", client: "efr-app 0.0.0", device_id: DEVICE_ID },
      },
    ]);
    socket.answer("hello", remoteHello());
    await flush();

    const fields = defined(proveFields(remoteHello(), DEVICE_ID));
    expect(key.signed.map((bytes) => hex(bytes))).toEqual([hex(proveMessage(fields))]);
    const [prove] = socket.requests("device.prove");
    expect(prove).toEqual({
      id: 2,
      method: "device.prove",
      params: { device_id: DEVICE_ID, signature: "MAYCASoCASs=" },
    });
    expect(base64Decode("MAYCASoCASs=")).toEqual(
      Uint8Array.of(0x30, 0x06, 0x02, 0x01, 0x2a, 0x02, 0x01, 0x2b),
    );

    socket.answer("device.prove", { scopes: ["read", "operate", "approve"] });
    const connection = await opening;
    expect(connection.scopes).toEqual(["read", "operate", "approve"]);
    expect(connection.hello.daemon_id).toBe(DAEMON_ID);
    expect(connection.closed).toBe(false);
  });

  it("is ready after hello when the phone pairs, and sends no device id", async () => {
    const parts = setup({}, true);
    const socket = parts.sockets.last;
    socket.open();
    await flush();
    expect(socket.sent[0]?.["params"]).toEqual({
      protocol: 1,
      origin: "phone",
      client: "efr-app 0.0.0",
    });
    socket.answer("hello", remoteHello());
    const connection = await parts.opening;
    expect(connection.scopes).toEqual([]);
    expect(socket.requests("device.prove")).toEqual([]);
  });

  it("pairs on one connection: hello, enroll, then the proof", async () => {
    const parts = setup({}, true);
    const socket = parts.sockets.last;
    socket.open();
    await flush();
    socket.answer("hello", remoteHello());
    const connection = await parts.opening;
    const enroll = connection.call("device.enroll", {
      token: "q83vASNFZ4mrze8BI0VniQ",
      name: "Anna's iPhone",
      public_key: "BAE=",
    });
    await flush();
    // efrd answers only after the terminal confirms; the probes keep the socket alive.
    await parts.clock.advance(DEFAULT_LIVENESS.probeAfterMs);
    socket.refuse("hello", "conflict", "hello was already sent on this connection");
    socket.answer("device.enroll", {
      device_id: DEVICE_ID,
      scopes: ["read", "operate", "approve"],
    });
    expect((await enroll).device_id).toBe(DEVICE_ID);

    const proving = connection.prove({ id: DEVICE_ID, key: parts.key });
    await flush();
    expect(socket.requests("device.prove")[0]?.["params"]).toMatchObject({ device_id: DEVICE_ID });
    socket.answer("device.prove", { scopes: ["read", "operate", "approve"] });
    expect(await proving).toEqual(["read", "operate", "approve"]);
    expect(connection.scopes).toEqual(["read", "operate", "approve"]);

    const refused = connection.prove({ id: DEVICE_ID, key: parts.key });
    await flush();
    socket.refuse("device.prove", "unauthorized", "bad signature");
    expect((await failure(refused)).kind).toBe("proof_refused");
    expect(connection.closed).toBe(false);
  });

  it("stops before the proof when another daemon or protocol answers", async () => {
    for (const [hello, kind] of [
      [remoteHello({ daemon_id: "019a9b1c-3d00-7a10-8b20-00000000ffff" }), "daemon_changed"],
      [{ ...remoteHello(), protocol: 2 }, "protocol_mismatch"],
      [remoteHello({ prove: null }), "no_prove_context"],
      [{ daemon_id: DAEMON_ID }, "malformed"],
    ] as const) {
      const { sockets, key, opening } = setup();
      sockets.last.open();
      await flush();
      sockets.last.answer("hello", hello);
      const error = await failure(opening);
      expect({ kind: error.kind, signed: key.signed.length }).toEqual({ kind, signed: 0 });
      expect(sockets.last.requests("device.prove")).toEqual([]);
      expect(sockets.last.closedWith).toBe(1000);
    }
  });

  it("maps the error answers of hello and the proof", async () => {
    const cases = [
      ["hello", "protocol_mismatch", undefined, "protocol_mismatch", "update", {}],
      ["hello", "busy", { retry_after_ms: 2500 }, "starting", "retry", { retryAfterMs: 2500 }],
      ["hello", "internal", undefined, "refused", "retry", {}],
      [
        "device.prove",
        "unauthorized",
        { re_pair: true, device_id: "019a9b1c-3d00-7a10-8b20-000000000006" },
        "proof_refused",
        "repair",
        { rePair: true },
      ],
      ["device.prove", "forbidden", undefined, "proof_refused", "repair", {}],
      [
        "device.prove",
        "busy",
        { retry_after_secs: 3 },
        "starting",
        "retry",
        { retryAfterMs: 3000 },
      ],
    ] as const;
    for (const [step, code, data, kind, recovery, details] of cases) {
      const { sockets, opening } = setup();
      const socket = sockets.last;
      socket.open();
      await flush();
      if (step === "device.prove") {
        socket.answer("hello", remoteHello());
        await flush();
      }
      socket.refuse(step, code, "no", data);
      const error = await failure(opening);
      const { retryAfterMs, rePair } = error.details;
      expect({
        step,
        code,
        kind: error.kind,
        recovery: error.recovery,
        retryAfterMs,
        rePair,
      }).toEqual({
        step,
        code,
        kind,
        recovery,
        retryAfterMs: undefined,
        rePair: undefined,
        ...details,
      });
    }
  });

  it("fails when the key cannot sign", async () => {
    const { sockets, key, opening } = setup();
    key.failing = true;
    sockets.last.open();
    await flush();
    sockets.last.answer("hello", remoteHello());
    const error = await failure(opening);
    expect([error.kind, error.recovery]).toEqual(["key_unavailable", "repair"]);
  });

  it("times out when the socket does not open or efrd does not answer", async () => {
    const silent = setup({ handshakeTimeoutMs: 15_000 });
    await silent.clock.advance(14_999);
    expect(silent.sockets.last.closedWith).toBeUndefined();
    await silent.clock.advance(1);
    expect((await failure(silent.opening)).kind).toBe("timeout");
    expect(silent.sockets.last.closedWith).toBe(1000);

    const mute = setup();
    mute.sockets.last.open();
    await mute.clock.advance(15_000);
    expect((await failure(mute.opening)).kind).toBe("timeout");
  });

  it("fails when the socket closes or efrd sends a bad frame during the handshake", async () => {
    const dropped = setup();
    dropped.sockets.last.drop(1001);
    const error = await failure(dropped.opening);
    expect([error.kind, error.details.closeCode]).toEqual(["socket", 1001]);
    expect(dropped.closes).toEqual([]);

    const garbled = setup();
    garbled.sockets.last.open();
    await flush();
    garbled.sockets.last.receiveRaw("{not json");
    expect((await failure(garbled.opening)).kind).toBe("malformed");

    const binary = setup();
    binary.sockets.last.open();
    await flush();
    binary.sockets.last.receiveRaw(new Uint8Array(1));
    expect((await failure(binary.opening)).kind).toBe("malformed");
  });

  it("fails when the socket factory throws", async () => {
    const parts = setup({
      sockets: () => {
        throw new Error("bad url");
      },
    });
    expect((await failure(parts.opening)).kind).toBe("socket");
  });
});

describe("requests", () => {
  it("resolves a call with its first item at the end", async () => {
    const { connection, socket } = await ready();
    const call = connection.call("conversations.list", { limit: 1 });
    await flush();
    const id = socket.lastId("conversations.list");
    socket.receive({ id, item: { conversations: [] } });
    socket.receive({ id, item: { conversations: ["ignored"] } });
    socket.receive({ id, end: true });
    expect(await call).toEqual({ conversations: [] });
  });

  it("rejects a call with the server error, or with no result", async () => {
    const { connection, socket } = await ready();
    const refused = connection.call("approval.respond", {
      command_id: "c",
      conversation_id: "x",
      call_id: "k",
      decision: "allow",
    });
    refused.catch(() => {});
    await flush();
    socket.refuse("approval.respond", "not_found", "no such approval");
    await expect(refused).rejects.toBeInstanceOf(ServerError);
    await expect(refused).rejects.toMatchObject({ body: { code: "not_found" } });

    const empty = connection.call("models.list", {});
    empty.catch(() => {});
    await flush();
    socket.receive({ id: socket.lastId("models.list"), end: true });
    await expect(empty).rejects.toMatchObject({ kind: "malformed" });
  });

  it("cancels a call after its timeout and ignores its late frames", async () => {
    const { connection, socket, clock } = await ready();
    const call = connection.call("models.list", {}, { timeoutMs: 5000 });
    call.catch(() => {});
    await flush();
    const id = socket.lastId("models.list");
    await clock.advance(5000);
    await expect(call).rejects.toMatchObject({ kind: "timeout" });
    expect(socket.sent.at(-1)).toEqual({ cancel: id });
    socket.answer("models.list", { models: [] });
    expect(connection.closed).toBe(false);
  });

  it("delivers stream items in order, then the end or the error", async () => {
    const { connection, socket } = await ready();
    const { log, handlers } = recorder();
    connection.stream("conversation.subscribe", { conversation_id: "c", drafts: true }, handlers);
    const id = socket.lastId("conversation.subscribe");
    expect(socket.requests("conversation.subscribe")[0]?.["params"]).toEqual({
      conversation_id: "c",
      drafts: true,
    });
    socket.receive({ ack: id });
    socket.receive({ id, item: { kind: "event", n: 1 } });
    socket.receive({ id, item: { kind: "event", n: 2 } });
    socket.receive({ id, error: { code: "overflow", message: "behind", data: { last_seq: 9 } } });
    socket.receive({ id, item: { late: true } });
    expect(log).toEqual([
      'item {"kind":"event","n":1}',
      'item {"kind":"event","n":2}',
      "error overflow",
    ]);
  });

  it("cancels a stream: a cancel frame, and no handler runs after it", async () => {
    const { connection, socket } = await ready();
    const { log, handlers } = recorder();
    const handle = connection.stream("conversation.subscribe", { conversation_id: "c" }, handlers);
    handle.cancel();
    handle.cancel();
    expect(socket.sent.filter((frame) => "cancel" in frame)).toEqual([{ cancel: handle.id }]);
    socket.receive({ id: handle.id, item: {} });
    socket.receive({ id: handle.id, error: { code: "cancelled", message: "x" } });
    expect(log).toEqual([]);
  });

  it("fails every pending request and stream when the socket closes", async () => {
    const { connection, socket, closes } = await ready();
    const { log, handlers } = recorder();
    connection.stream("conversation.subscribe", { conversation_id: "c" }, handlers);
    const call = connection.call("models.list", {});
    call.catch(() => {});
    await flush();
    socket.drop(1001);
    await expect(call).rejects.toMatchObject({ kind: "socket" });
    expect(log).toEqual(["lost socket"]);
    expect(closes.map((error) => [error.kind, error.details.closeCode])).toEqual([
      ["socket", 1001],
    ]);
    expect(connection.closed).toBe(true);

    const late = recorder();
    connection.stream("conversation.subscribe", { conversation_id: "d" }, late.handlers);
    await flush();
    expect(late.log).toEqual(["lost socket"]);
    await expect(connection.call("models.list", {})).rejects.toMatchObject({ kind: "socket" });
  });

  it("closes on an error frame without an id", async () => {
    const { connection, socket, closes } = await ready();
    socket.receive({ error: { code: "invalid", message: "bad frame" } });
    expect(connection.closed).toBe(true);
    expect(closes.map((error) => error.kind)).toEqual(["refused"]);
  });

  it("closes on purpose with kind closed and calls onClose once", async () => {
    const { connection, socket, closes } = await ready();
    connection.close();
    connection.close();
    socket.drop();
    expect(socket.closedWith).toBe(1000);
    expect(closes.map((error) => [error.kind, error.recovery])).toEqual([["closed", "none"]]);
  });
});

describe("the liveness rule", () => {
  it("probes with a second hello after the probe silence, and stays up on the answer", async () => {
    const { connection, socket, clock } = await ready();
    await clock.advance(DEFAULT_LIVENESS.probeAfterMs - 1);
    expect(socket.requests("hello")).toHaveLength(1);
    await clock.advance(1);
    const probes = socket.requests("hello");
    expect(probes).toHaveLength(2);
    expect(probes[1]?.["params"]).toEqual({
      protocol: 1,
      origin: "phone",
      client: "efr-app 0.0.0",
    });
    // The transport of efrd answers a second hello with `conflict`.
    socket.refuse("hello", "conflict", "hello was already sent on this connection");
    // The answer came in the same millisecond as the probe; it still counts, so the next
    // silence probes again instead of closing.
    await clock.advance(DEFAULT_LIVENESS.probeAfterMs);
    expect(socket.requests("hello")).toHaveLength(3);
    socket.refuse("hello", "conflict", "hello was already sent on this connection");
    await clock.advance(DEFAULT_LIVENESS.deadAfterMs - 1);
    expect(connection.closed).toBe(false);
  });

  it("counts every decoded frame, such as stream items, as life", async () => {
    const { connection, socket, clock } = await ready();
    const { handlers } = recorder();
    const handle = connection.stream("conversation.subscribe", { conversation_id: "c" }, handlers);
    for (let i = 0; i < 10; i++) {
      await clock.advance(5000);
      socket.receive({ id: handle.id, item: { kind: "draft" } });
    }
    expect(socket.requests("hello")).toHaveLength(1);
    expect(connection.closed).toBe(false);
  });

  it("closes a silent connection after the dead silence, once", async () => {
    const { connection, socket, clock, closes } = await ready();
    await clock.advance(DEFAULT_LIVENESS.deadAfterMs - 1);
    expect(connection.closed).toBe(false);
    expect(socket.requests("hello")).toHaveLength(2);
    await clock.advance(1);
    expect(connection.closed).toBe(true);
    expect(socket.closedWith).toBe(4000);
    expect(closes.map((error) => [error.kind, error.recovery])).toEqual([["silent", "retry"]]);
    await clock.advance(60_000);
    expect(socket.requests("hello")).toHaveLength(2);
    expect(clock.pendingTimers).toBe(0);
  });
});
