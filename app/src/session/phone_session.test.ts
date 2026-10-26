import { describe, expect, it } from "vitest";

import { ConnectionError } from "@/lib/net/errors";
import {
  DAEMON_ID,
  DEVICE_ID,
  FakeClock,
  FakeSockets,
  completeHandshake,
  flush,
  remoteHello,
  type FakeSocket,
} from "@/lib/net/testing";
import { deviceKeyStore } from "@/pairing/device_key";
import type { PairingDeps } from "@/pairing/pair";
import type { PairedDevice } from "@/pairing/paired_device";
import type { ScannerApi } from "@/pairing/scanner";
import { FakeKeyBinding, MemoryPairedDevices } from "@/pairing/testing";
import type { ConversationSummary } from "@/lib/protocol/generated";
import { MemoryEventCache } from "@/lib/store/event-cache";
import { CALL, CONVERSATION, rawEvent, rawItem } from "@/lib/protocol/testing";
import type { ConnectionView, PairingView } from "@/ui/session";
import { connectionView, createPhoneSession, pairingFailure, repairReason } from "./phone_session";

const HOST = "100.101.102.103";
const PORT = 7419;
const TOKEN = "q83vASNFZ4mrze8BI0VniQ";
const CODE = `efr://pair?v=1&d=${DAEMON_ID}&h=${HOST}&p=${PORT}&t=${TOKEN}`;

const SUMMARY: ConversationSummary = {
  id: CONVERSATION,
  title: "why is the disk full",
  status: "awaiting_approval",
  created_at: "2026-10-26T10:00:00Z",
  updated_at: "2026-10-26T12:00:00Z",
  last_seq: 3,
  cwd: "/var/log",
};

/** The system scanner in memory: `show` shows it a code. */
class FakeScanner implements ScannerApi {
  available = true;
  granted = true;
  private listener: ((data: string) => void) | undefined;

  requestPermission(): Promise<boolean> {
    return Promise.resolve(this.granted);
  }

  launch(): Promise<void> {
    return Promise.resolve();
  }

  dismiss(): Promise<void> {
    return Promise.resolve();
  }

  onScanned(listener: (data: string) => void): { remove(): void } {
    this.listener = listener;
    return { remove: () => (this.listener = undefined) };
  }

  show(data: string): void {
    this.listener?.(data);
  }
}

function setup() {
  const clock = new FakeClock(Date.parse("2026-10-26T12:00:00Z"));
  const sockets = new FakeSockets();
  const binding = new FakeKeyBinding();
  const devices = new MemoryPairedDevices();
  const scanner = new FakeScanner();
  const cache = new MemoryEventCache();
  let ids = 0;
  let aliases = 0;
  const pairing: PairingDeps = {
    sockets: sockets.factory,
    clock,
    keys: deviceKeyStore(binding),
    devices,
    sha256: () => Promise.resolve(Uint8Array.from({ length: 32 }, (_, i) => i)),
    newKeyAlias: () => `device-${++aliases}`,
    now: () => new Date(clock.now()),
    client: "efr-app test",
  };
  const followed: string[] = [];
  const session = createPhoneSession({
    pairing,
    scanner,
    deviceName: () => "Anna's iPhone",
    sockets: sockets.factory,
    clock,
    random: () => 0,
    client: "efr-app test",
    openCache: () => Promise.resolve(cache),
    newCommandId: () => `019a9b1c-3d00-7a10-8b20-0000000001${String(++ids).padStart(2, "0")}`,
    follow: () => {
      followed.push("on");
      return () => followed.push("off");
    },
  });
  return { clock, sockets, binding, devices, scanner, cache, session, followed };
}

/** A pairing record and its key, as an earlier pairing left them. */
async function paired(binding: FakeKeyBinding, devices: MemoryPairedDevices): Promise<void> {
  await deviceKeyStore(binding).create("device-old");
  const device: PairedDevice = {
    deviceId: DEVICE_ID,
    daemonId: DAEMON_ID,
    host: HOST,
    port: PORT,
    name: "Anna's iPhone",
    keyAlias: "device-old",
    scopes: ["read", "operate", "approve"],
    pairedAt: "2026-10-01T00:00:00.000Z",
  };
  devices.device = device;
}

/** Starts a paired session up to its first list request. */
async function online() {
  const setupResult = setup();
  const { binding, devices, sockets, session } = setupResult;
  await paired(binding, devices);
  await session.start();
  await completeHandshake(sockets.last);
  return { ...setupResult, socket: sockets.last };
}

function answerList(socket: FakeSocket): void {
  socket.answer("conversations.list", { conversations: [SUMMARY] });
}

describe("the phone session", () => {
  it("is unpaired without a pairing record and opens no socket", async () => {
    const { session, sockets } = setup();
    expect(session.connection.getSnapshot()).toEqual({ kind: "connecting", endpoint: "" });
    await session.start();
    expect(session.connection.getSnapshot()).toEqual({ kind: "unpaired" });
    expect(sockets.made.length).toBe(0);
  });

  it("connects with the paired device, proves its key and lists the conversations", async () => {
    const { session, socket, followed } = await online();
    expect(socket.url).toBe(`ws://${HOST}:${PORT}/`);
    expect(socket.requests("hello")[0]?.["params"]).toMatchObject({ device_id: DEVICE_ID });
    expect(socket.requests("device.prove")[0]?.["params"]).toMatchObject({ device_id: DEVICE_ID });
    expect(session.connection.getSnapshot()).toEqual({
      kind: "online",
      endpoint: `${HOST}:${PORT}`,
    });
    expect(followed).toEqual(["on"]);

    answerList(socket);
    await flush();
    expect(session.conversations.getSnapshot()).toEqual([
      {
        id: CONVERSATION,
        title: "why is the disk full",
        status: "awaiting_approval",
        updatedAt: Date.parse("2026-10-26T12:00:00Z"),
        cwd: "/var/log",
      },
    ]);
    session.dispose();
    expect(followed).toEqual(["on", "off"]);
  });

  it("follows a conversation exactly while a screen listens to it", async () => {
    const { session, socket } = await online();
    answerList(socket);
    const source = session.conversation(CONVERSATION);
    expect(session.conversation(CONVERSATION)).toBe(source);
    await flush();
    expect(socket.requests("conversation.subscribe")).toEqual([]);

    const offStream = source.stream.subscribe(() => {});
    const offDetails = source.details.subscribe(() => {});
    await flush();
    const [subscribe] = socket.requests("conversation.subscribe");
    expect(subscribe?.["params"]).toEqual({ conversation_id: CONVERSATION, drafts: true });
    const id = subscribe?.["id"];
    socket.receive({ id, item: rawItem("snapshot") });
    socket.receive({ id, item: rawEvent("approval_requested", 43) });
    socket.receive({ id, item: rawItem("draft") });
    await flush();
    expect(source.title.getSnapshot()).toBe("why is the disk full");
    expect(source.stream.getSnapshot().lastSeq).toBe(43);
    const turn = source.details.getSnapshot().turns.get(rawTurn());
    expect(turn?.approvals.get(CALL)?.state).toBe("pending");

    offStream();
    offDetails();
    await flush();
    expect(socket.sent.some((frame) => frame["cancel"] === id)).toBe(true);
  });

  it("sends a prompt and an answer with command ids, and says why efrd refused", async () => {
    const { session, socket } = await online();
    const source = session.conversation(CONVERSATION);
    const off = source.stream.subscribe(() => {});
    await flush();
    const id = socket.lastId("conversation.subscribe");
    socket.receive({ id, item: rawItem("snapshot") });
    socket.receive({ id, item: rawEvent("approval_requested", 43) });
    await flush();

    const sent = session.sendPrompt(CONVERSATION, "and the logs?");
    await flush();
    expect(socket.requests("prompt.send")[0]?.["params"]).toEqual({
      command_id: "019a9b1c-3d00-7a10-8b20-000000000101",
      conversation_id: CONVERSATION,
      text: "and the logs?",
    });
    socket.answer("prompt.send", { conversation_id: CONVERSATION, turn_id: rawTurn(), seq: 44 });
    expect(await sent).toEqual({ ok: true });

    const answered = session.answerApproval(CONVERSATION, CALL, "allow");
    await flush();
    expect(socket.requests("approval.respond")[0]?.["params"]).toEqual({
      command_id: "019a9b1c-3d00-7a10-8b20-000000000102",
      conversation_id: CONVERSATION,
      call_id: CALL,
      decision: "allow",
    });
    socket.refuse("approval.respond", "forbidden", "only the terminal may answer this");
    expect(await answered).toEqual({
      ok: false,
      message: "Only the terminal may answer this.",
    });
    expect(await session.answerApproval(CONVERSATION, "nope", "deny")).toEqual({
      ok: false,
      message: "The phone does not know this question yet.",
    });
    off();
  });

  it("asks to pair again when efrd refuses the proof, and forgets the pairing", async () => {
    const { session, sockets, binding, devices } = setup();
    await paired(binding, devices);
    await session.start();
    const socket = sockets.last;
    socket.open();
    await flush();
    socket.answer("hello", remoteHello());
    await flush();
    socket.refuse("device.prove", "unauthorized", "the device could not prove its key", {
      re_pair: true,
      device_id: DEVICE_ID,
    });
    await flush();
    expect(session.connection.getSnapshot()).toEqual({
      kind: "repair_needed",
      reason: "node_changed",
    });

    await session.forget();
    expect(session.connection.getSnapshot()).toEqual({ kind: "unpaired" });
    expect(devices.device).toBeNull();
    expect(binding.keys.size).toBe(0);
  });

  it("pairs from a scanned code and is online when the flow says paired", async () => {
    const { session, sockets, scanner, devices } = setup();
    await session.start();
    const views: PairingView[] = [];
    session.pairing.state.subscribe(() => views.push(session.pairing.state.getSnapshot()));
    session.pairing.start();
    await flush();
    scanner.show("https://example.com");
    scanner.show(CODE);
    await flush();
    const pairingSocket = sockets.last;
    pairingSocket.open();
    await flush();
    pairingSocket.answer("hello", remoteHello());
    await flush();
    expect(pairingSocket.requests("device.enroll")[0]?.["params"]).toMatchObject({
      token: TOKEN,
      name: "Anna's iPhone",
    });
    pairingSocket.answer("device.enroll", {
      device_id: DEVICE_ID,
      scopes: ["read", "operate", "approve"],
    });
    await flush();

    expect(views).toEqual([
      { kind: "scanning" },
      { kind: "connecting", host: HOST },
      { kind: "confirm", host: HOST, deviceName: "Anna's iPhone" },
      { kind: "paired" },
    ]);
    expect(devices.device?.deviceId).toBe(DEVICE_ID);
    expect(pairingSocket.closedWith).toBeDefined();
    const client = sockets.last;
    expect(client).not.toBe(pairingSocket);
    await completeHandshake(client);
    expect(session.connection.getSnapshot()).toMatchObject({ kind: "online" });
  });

  it("names a camera refusal and a cancel of the scan", async () => {
    const { session, scanner } = setup();
    scanner.granted = false;
    session.pairing.start();
    await flush();
    expect(session.pairing.state.getSnapshot()).toEqual({ kind: "failed", reason: "camera" });
    scanner.granted = true;
    session.pairing.start();
    await flush();
    session.pairing.cancel();
    await flush();
    expect(session.pairing.state.getSnapshot()).toEqual({ kind: "idle" });
  });

  it("shows only the newest pairing flow", async () => {
    const { session } = setup();
    session.pairing.start();
    await flush();
    session.pairing.start();
    await flush();
    expect(session.pairing.state.getSnapshot()).toEqual({ kind: "scanning" });
    session.pairing.cancel();
    await flush();
    expect(session.pairing.state.getSnapshot()).toEqual({ kind: "idle" });
  });

  it("refuses to pair without a device key module", async () => {
    const session = createPhoneSession({
      pairing: null,
      scanner: new FakeScanner(),
      deviceName: () => "iPhone",
      sockets: new FakeSockets().factory,
      clock: new FakeClock(),
      random: () => 0,
      client: "efr-app test",
      newCommandId: () => "id",
    });
    await session.start();
    expect(session.connection.getSnapshot()).toEqual({ kind: "unpaired" });
    session.pairing.start();
    expect(session.pairing.state.getSnapshot()).toMatchObject({ kind: "failed", reason: "other" });
    expect(await session.sendPrompt(CONVERSATION, "hi")).toEqual({
      ok: false,
      message: "The phone is not paired.",
    });
  });
});

describe("connectionView", () => {
  const endpoint = `${HOST}:${PORT}`;
  const waiting = (error: ConnectionError): ConnectionView =>
    connectionView({ state: "waiting", retryAt: 5_000, error }, endpoint);
  const stopped = (error: ConnectionError): ConnectionView =>
    connectionView({ state: "stopped", error }, endpoint);

  it("tells Tailscale, a starting efrd and a closed connection apart", () => {
    expect(waiting(new ConnectionError("socket", "x"))).toEqual({
      kind: "unreachable",
      endpoint,
      retryAt: 5_000,
    });
    expect(waiting(new ConnectionError("timeout", "x")).kind).toBe("unreachable");
    expect(waiting(new ConnectionError("starting", "x")).kind).toBe("starting");
    expect(waiting(new ConnectionError("silent", "x")).kind).toBe("retrying");
    expect(connectionView({ state: "connecting" }, endpoint)).toEqual({
      kind: "connecting",
      endpoint,
    });
  });

  it("asks to pair again or to update when the client stopped", () => {
    expect(stopped(new ConnectionError("protocol_mismatch", "x"))).toEqual({
      kind: "update_needed",
    });
    expect(stopped(new ConnectionError("proof_refused", "x"))).toEqual({
      kind: "repair_needed",
      reason: "unknown_device",
    });
    expect(stopped(new ConnectionError("daemon_changed", "x"))).toEqual({
      kind: "repair_needed",
      reason: "other",
    });
  });

  it("reads re_pair as a changed node", () => {
    expect(repairReason(new ConnectionError("proof_refused", "x", { rePair: true }))).toBe(
      "node_changed",
    );
  });
});

describe("pairingFailure", () => {
  it("names each failure of the flow for the screen", () => {
    expect(pairingFailure({ kind: "cancelled" })).toEqual({ kind: "idle" });
    expect(pairingFailure({ kind: "not_efr" })).toEqual({ kind: "failed", reason: "not_a_code" });
    expect(pairingFailure({ kind: "protocol_mismatch" })).toEqual({
      kind: "failed",
      reason: "wrong_daemon",
    });
    expect(pairingFailure({ kind: "token_refused", message: "no" })).toEqual({
      kind: "failed",
      reason: "expired",
    });
    expect(pairingFailure({ kind: "failed", message: "the store failed" })).toEqual({
      kind: "failed",
      reason: "other",
      message: "The pairing failed: the store failed.",
    });
  });
});

/** The turn of the fixtures. */
function rawTurn(): string {
  return "019a9b1c-3d00-7a10-8b20-000000000002";
}
