import { describe, expect, it } from "vitest";

import { ConnectionError, OPEN_TIMEOUT_MS, PairingConnection, RpcError } from "./connection";
import { FakeTimers, fakeSockets, settle } from "./testing";

async function opened() {
  const timers = new FakeTimers();
  const { open, sockets } = fakeSockets();
  const pending = PairingConnection.open(open, "ws://100.64.0.1:7878/", timers.schedule);
  const socket = sockets[0];
  if (socket === undefined) {
    throw new Error("no socket");
  }
  socket.open();
  return { connection: await pending, socket, timers };
}

describe("PairingConnection.open", () => {
  it("opens the URL and stops its timer once the socket is up", async () => {
    const { socket, timers } = await opened();
    expect(socket.url).toBe("ws://100.64.0.1:7878/");
    expect(timers.pending).toBe(0);
  });

  it("fails as unreachable when the socket does not come up in time", async () => {
    const timers = new FakeTimers();
    const { open, sockets } = fakeSockets();
    const pending = PairingConnection.open(open, "ws://100.64.0.1:7878/", timers.schedule);
    timers.advance(OPEN_TIMEOUT_MS - 1);
    await settle();
    timers.advance(1);
    await expect(pending).rejects.toMatchObject({ problem: "unreachable" });
    expect(sockets[0]?.closedByClient).toBe(true);
    // A late open changes nothing.
    sockets[0]?.open();
  });

  it("fails as unreachable when the socket closes before it opens", async () => {
    const timers = new FakeTimers();
    const { open, sockets } = fakeSockets();
    const pending = PairingConnection.open(open, "ws://100.64.0.1:7878/", timers.schedule);
    sockets[0]?.close(1006, "");
    await expect(pending).rejects.toBeInstanceOf(ConnectionError);
    await expect(pending).rejects.toMatchObject({ problem: "unreachable" });
    expect(timers.pending).toBe(0);
  });
});

describe("PairingConnection.call", () => {
  it("sends a request frame and gives the item of its end", async () => {
    const { connection, socket } = await opened();
    const result = connection.call("hello", { protocol: 1 });
    expect(socket.requests).toEqual([{ id: 1, method: "hello", params: { protocol: 1 } }]);
    socket.frame({ ack: 1 });
    socket.result(1, { daemon_id: "d" });
    await expect(result).resolves.toEqual({ daemon_id: "d" });
  });

  it("numbers the requests and matches each answer by its id", async () => {
    const { connection, socket } = await opened();
    const one = connection.call("hello", {});
    const two = connection.call("device.enroll", {});
    socket.result(2, "second");
    socket.result(99, "nobody");
    socket.result(1, "first");
    await expect(one).resolves.toBe("first");
    await expect(two).resolves.toBe("second");
  });

  it("fails with the code, message and data of an error frame", async () => {
    const { connection, socket } = await opened();
    const result = connection.call("device.enroll", {});
    socket.error(1, "unauthorized", "the token is not valid", { left: 2 });
    const error = await result.catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(RpcError);
    expect(error).toMatchObject({
      code: "unauthorized",
      message: "the token is not valid",
      data: { left: 2 },
    });
    expect(connection.isClosed).toBe(false);
  });

  it("fails as protocol for an end without a result, or two results", async () => {
    const { connection, socket } = await opened();
    const none = connection.call("hello", {});
    socket.frame({ id: 1, end: true });
    await expect(none).rejects.toMatchObject({ problem: "protocol" });
    const two = connection.call("hello", {});
    socket.frame({ id: 2, item: 1 });
    socket.frame({ id: 2, item: 2 });
    await expect(two).rejects.toMatchObject({ problem: "protocol" });
  });

  it.each([
    ["text that is not JSON", "not json"],
    ["a frame without a kind", JSON.stringify({ id: 1 })],
    ["a message that is not text", 7],
  ])("ends the connection on %s", async (_, data: unknown) => {
    const { connection, socket } = await opened();
    const result = connection.call("hello", {});
    socket.handlers.message(data);
    await expect(result).rejects.toMatchObject({ problem: "protocol" });
    expect(connection.isClosed).toBe(true);
    expect(socket.closedByClient).toBe(true);
    await expect(connection.call("hello", {})).rejects.toMatchObject({ problem: "protocol" });
  });

  it("ends the connection on an error without an id", async () => {
    const { connection, socket } = await opened();
    const result = connection.call("hello", {});
    socket.error(null, "invalid", "the frame is not valid JSON");
    await expect(result).rejects.toMatchObject({
      problem: "protocol",
      message: "the frame is not valid JSON",
    });
  });

  it("fails each waiting request as closed when the socket closes", async () => {
    const { connection, socket } = await opened();
    const one = connection.call("hello", {});
    const two = connection.call("device.enroll", {});
    socket.close(1001, "going away");
    await expect(one).rejects.toMatchObject({ problem: "closed" });
    await expect(two).rejects.toMatchObject({ problem: "closed" });
    await expect(connection.call("hello", {})).rejects.toMatchObject({ problem: "closed" });
  });

  it("fails each waiting request as closed when the app closes it", async () => {
    const { connection, socket } = await opened();
    const result = connection.call("device.enroll", {});
    connection.close();
    await expect(result).rejects.toMatchObject({ problem: "closed" });
    expect(socket.closedByClient).toBe(true);
    // A frame after the close changes nothing.
    socket.result(1, "late");
  });
});
