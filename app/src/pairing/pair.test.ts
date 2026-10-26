import { describe, expect, it } from "vitest";

import { decodeBase64 } from "./base64";
import type { OpenSocket } from "./connection";
import { deviceKeyStore } from "./device_key";
import { cancellation, pair, PROTOCOL_VERSION } from "./pair";
import type { PairingDeps, PairingStep } from "./pair";
import type { PairedDevice } from "./paired_device";
import {
  FakeKeyBinding,
  FakeSocket,
  FakeTimers,
  fakeSockets,
  later,
  MemoryPairedDevices,
  parseRequest,
  settle,
} from "./testing";
import { isJsonObject } from "./json";

const DAEMON = "019a9b1c-3d00-7a10-8b20-000000000007";
const DEVICE_ID = "019a9b1c-3d00-7a10-8b20-000000000006";
const TOKEN = "q83vASNFZ4mrze8BI0VniQ";
const CODE = `efr://pair?v=1&d=${DAEMON}&h=100.101.102.103&p=7878&f=desk.tail1234.ts.net&t=${TOKEN}`;
const CODE_WITHOUT_NAME = `efr://pair?v=1&d=${DAEMON}&h=100.101.102.103&p=7878&t=${TOKEN}`;

const HELLO = {
  daemon_id: DAEMON,
  protocol: 1,
  version: "0.0.6",
  capabilities: { device_prove: true },
  paths: { scratch_root: "/s", data_dir: "/d" },
  challenge: "3q2-7wAAAAAAAAAAAAAAAA",
  prove: { peer_node: "nPHONE", daemon_node: "nDESK", daemon_tailnet: "T1" },
};

const ENROLLED = { device_id: DEVICE_ID, scopes: ["read", "operate", "approve"] };

type Answer =
  | { readonly item: unknown }
  | { readonly error: { code: string; message: string; data?: unknown } }
  | "hang";

/**
 * A fake efrd behind fake sockets: a socket to a reachable URL opens on the next job, and
 * each request gets the answer of `answer`.
 */
function fakeDaemon(
  answer: (method: string, params: unknown) => Answer,
  reachable: (url: string) => boolean = () => true,
): { readonly open: OpenSocket; readonly sockets: FakeSocket[] } {
  const sockets: FakeSocket[] = [];
  const open: OpenSocket = (url, handlers) => {
    const socket = new FakeSocket(url, handlers);
    sockets.push(socket);
    void later(() => (reachable(url) ? socket.open() : socket.close(1006, "")));
    return {
      send: (text) => {
        socket.sent.push(text);
        const request = parseRequest(text);
        void later(() => {
          const reply = answer(request.method, request.params);
          if (socket.closedByClient || reply === "hang") {
            return;
          }
          if ("item" in reply) {
            socket.result(request.id, reply.item);
          } else {
            socket.frame({ id: request.id, error: reply.error });
          }
        });
      },
      close: () => {
        socket.closedByClient = true;
      },
    };
  };
  return { open, sockets };
}

function answers(overrides: Partial<Record<string, Answer>> = {}): (method: string) => Answer {
  return (method) =>
    overrides[method] ?? (method === "hello" ? { item: HELLO } : { item: ENROLLED });
}

function setup(open: OpenSocket) {
  const binding = new FakeKeyBinding();
  const devices = new MemoryPairedDevices();
  const timers = new FakeTimers();
  let aliases = 0;
  const deps: PairingDeps = {
    openSocket: open,
    schedule: timers.schedule,
    keys: deviceKeyStore(binding),
    devices,
    sha256: () => Promise.resolve(Uint8Array.from({ length: 32 }, (_, i) => i)),
    newKeyAlias: () => `device-${++aliases}`,
    now: () => new Date("2026-10-26T09:01:00Z"),
    client: "efr-app test",
  };
  return { binding, devices, timers, deps };
}

describe("pair", () => {
  it("checks the daemon, enrolls the key and keeps the record", async () => {
    const daemon = fakeDaemon(answers());
    const { binding, devices, deps } = setup(daemon.open);
    const steps: PairingStep[] = [];
    const result = await pair(CODE, deps, {
      name: "  Anna's\niPhone ",
      onStep: (step) => steps.push(step),
    });

    const expected: PairedDevice = {
      deviceId: DEVICE_ID,
      daemonId: DAEMON,
      host: "100.101.102.103",
      port: 7878,
      fqdn: "desk.tail1234.ts.net",
      name: "Anna's iPhone",
      keyAlias: "device-1",
      scopes: ["read", "operate", "approve"],
      pairedAt: "2026-10-26T09:01:00.000Z",
    };
    expect(result).toEqual({ ok: true, device: expected });
    expect(devices.device).toEqual(expected);
    expect(steps).toEqual([
      { step: "connecting", url: "ws://100.101.102.103:7878/" },
      { step: "checking" },
      { step: "confirm", name: "Anna's iPhone", fingerprint: "SHA256:0001020304050607" },
      { step: "saving" },
    ]);

    const socket = daemon.sockets[0];
    expect(daemon.sockets.length).toBe(1);
    const key = binding.keys.get("device-1");
    expect(socket?.requests).toEqual([
      {
        id: 1,
        method: "hello",
        params: {
          protocol: PROTOCOL_VERSION,
          origin: "phone",
          client: "efr-app test",
          capabilities: {},
        },
      },
      {
        id: 2,
        method: "device.enroll",
        params: { token: TOKEN, name: "Anna's iPhone", public_key: expect.any(String) as unknown },
      },
    ]);
    const sent = socket?.requests[1]?.params;
    const publicKey = isJsonObject(sent) ? sent["public_key"] : undefined;
    expect(typeof publicKey === "string" && decodeBase64(publicKey)).toEqual(key);
    expect(socket?.closedByClient).toBe(true);
  });

  it("removes the key of the earlier pairing once the new record is kept", async () => {
    const { binding, devices, deps } = setup(fakeDaemon(answers()).open);
    await deps.keys.create("device-old");
    devices.device = {
      deviceId: "019a9b1c-3d00-7a10-8b20-0000000000ff",
      daemonId: DAEMON,
      host: "100.101.102.103",
      port: 7878,
      name: "old",
      keyAlias: "device-old",
      scopes: ["read"],
      pairedAt: "2026-10-01T00:00:00.000Z",
    };
    const result = await pair(CODE, deps, { name: "iPhone" });
    expect(result.ok).toBe(true);
    expect([...binding.keys.keys()]).toEqual(["device-1"]);
    expect(devices.device?.keyAlias).toBe("device-1");
  });

  it("stops before the token goes out when the daemon is not the one of the code", async () => {
    const other = "019a9b1c-3d00-7a10-8b20-0000000000aa";
    const daemon = fakeDaemon(answers({ hello: { item: { ...HELLO, daemon_id: other } } }));
    const { binding, devices, deps } = setup(daemon.open);
    const result = await pair(CODE, deps, { name: "iPhone" });
    expect(result).toEqual({ ok: false, failure: { kind: "wrong_daemon", daemonId: other } });
    expect(daemon.sockets[0]?.requests.map((request) => request.method)).toEqual(["hello"]);
    expect(daemon.sockets[0]?.sent.join("")).not.toContain(TOKEN);
    expect(binding.keys.size).toBe(0);
    expect(devices.device).toBeNull();
    expect(daemon.sockets[0]?.closedByClient).toBe(true);
  });

  it("stops before the token goes out on another protocol version", async () => {
    const newer = fakeDaemon(answers({ hello: { item: { ...HELLO, protocol: 2 } } }));
    const one = await pair(CODE, setup(newer.open).deps, { name: "iPhone" });
    expect(one).toEqual({ ok: false, failure: { kind: "protocol_mismatch", daemon: 2 } });
    expect(newer.sockets[0]?.sent.join("")).not.toContain(TOKEN);

    const refused = fakeDaemon(
      answers({
        hello: {
          error: { code: "protocol_mismatch", message: "no", data: { daemon: 3, client: 1 } },
        },
      }),
    );
    const two = await pair(CODE, setup(refused.open).deps, { name: "iPhone" });
    expect(two).toEqual({ ok: false, failure: { kind: "protocol_mismatch", daemon: 3 } });
  });

  it.each([
    ["unauthorized", "the token is not valid", "token_refused"],
    ["forbidden", "the user said no", "refused"],
    ["internal", "the store failed", "failed"],
  ] as const)(
    "names an enroll error %s as %s and removes the new key",
    async (code, message, kind) => {
      const { binding, devices, deps } = setup(
        fakeDaemon(answers({ "device.enroll": { error: { code, message } } })).open,
      );
      const result = await pair(CODE, deps, { name: "iPhone" });
      expect(result).toEqual({ ok: false, failure: { kind, message } });
      expect(binding.keys.size).toBe(0);
      expect(devices.device).toBeNull();
    },
  );

  it("tries the MagicDNS name when the tailnet address does not answer", async () => {
    const daemon = fakeDaemon(answers(), (url) => url.includes("ts.net"));
    const { deps } = setup(daemon.open);
    const result = await pair(CODE, deps, { name: "iPhone" });
    expect(daemon.sockets.map((socket) => socket.url)).toEqual([
      "ws://100.101.102.103:7878/",
      "ws://desk.tail1234.ts.net:7878/",
    ]);
    expect(result.ok && result.device.host).toBe("100.101.102.103");
  });

  it("is unreachable when no address answers", async () => {
    const both = fakeDaemon(answers(), () => false);
    expect(await pair(CODE, setup(both.open).deps, { name: "iPhone" })).toEqual({
      ok: false,
      failure: { kind: "unreachable" },
    });
    expect(both.sockets.length).toBe(2);

    const one = fakeDaemon(answers(), () => false);
    expect(await pair(CODE_WITHOUT_NAME, setup(one.open).deps, { name: "iPhone" })).toEqual({
      ok: false,
      failure: { kind: "unreachable" },
    });
    expect(one.sockets.length).toBe(1);
  });

  it("is unreachable when the socket never comes up", async () => {
    const sockets: FakeSocket[] = [];
    const silent: OpenSocket = (url, handlers) => {
      sockets.push(new FakeSocket(url, handlers));
      return { send: () => {}, close: () => {} };
    };
    const { deps, timers } = setup(silent);
    const result = pair(CODE_WITHOUT_NAME, deps, { name: "iPhone" });
    await settle();
    timers.advance(10_000);
    expect(await result).toEqual({ ok: false, failure: { kind: "unreachable" } });
  });

  it("cancels while it waits for the terminal, closes the socket and removes the key", async () => {
    const daemon = fakeDaemon(answers({ "device.enroll": "hang" }));
    const { binding, devices, deps } = setup(daemon.open);
    const { signal, cancel } = cancellation();
    const result = await pair(CODE, deps, {
      name: "iPhone",
      signal,
      onStep: (step) => {
        if (step.step === "confirm") {
          cancel();
        }
      },
    });
    expect(result).toEqual({ ok: false, failure: { kind: "cancelled" } });
    expect(daemon.sockets[0]?.closedByClient).toBe(true);
    await settle();
    expect(binding.keys.size).toBe(0);
    expect(devices.device).toBeNull();
  });

  it("cancels while the socket opens and closes the socket once it is up", async () => {
    const { open, sockets } = fakeSockets();
    const { deps } = setup(open);
    const { signal, cancel } = cancellation();
    const result = pair(CODE, deps, {
      name: "iPhone",
      signal,
      onStep: (step) => {
        if (step.step === "connecting") {
          cancel();
        }
      },
    });
    expect(await result).toEqual({ ok: false, failure: { kind: "cancelled" } });
    // The socket comes up only after the flow ended.
    sockets[0]?.open();
    await settle();
    expect(sockets[0]?.closedByClient).toBe(true);
    expect(sockets[0]?.sent).toEqual([]);
  });

  it("removes a key that the store makes after a cancel", async () => {
    const daemon = fakeDaemon(answers());
    const { binding, deps } = setup(daemon.open);
    const create = binding.createKey.bind(binding);
    let finishCreate: (() => void) | undefined;
    const asked = new Promise<void>((askedResolve) => {
      binding.createKey = (alias) =>
        new Promise((resolve) => {
          finishCreate = () => resolve(create(alias));
          askedResolve();
        });
    });
    const { signal, cancel } = cancellation();
    const result = pair(CODE, deps, { name: "iPhone", signal });
    await asked;
    cancel();
    expect(await result).toEqual({ ok: false, failure: { kind: "cancelled" } });
    finishCreate?.();
    await settle();
    expect(binding.keys.size).toBe(0);
  });

  it("names a failure of the key store", async () => {
    const { binding, deps } = setup(fakeDaemon(answers()).open);
    binding.failCreate = new Error("This iPhone has no Secure Enclave");
    expect(await pair(CODE, deps, { name: "iPhone" })).toEqual({
      ok: false,
      failure: { kind: "key_failed", message: "This iPhone has no Secure Enclave" },
    });
  });

  it("removes the new key when the record cannot be kept", async () => {
    const { binding, devices, deps } = setup(fakeDaemon(answers()).open);
    devices.failSave = new Error("the keychain is locked");
    const result = await pair(CODE, deps, { name: "iPhone" });
    expect(result).toEqual({
      ok: false,
      failure: {
        kind: "failed",
        message: "the phone could not keep the pairing: the keychain is locked",
      },
    });
    expect(binding.keys.size).toBe(0);
  });

  it.each([
    ["a hello without daemon_id", { hello: { item: { protocol: 1 } } }],
    ["a hello that is not an object", { hello: { item: "hello" } }],
    [
      "an enroll with a bad device_id",
      { "device.enroll": { item: { device_id: "nope", scopes: [] } } },
    ],
    ["an enroll without scopes", { "device.enroll": { item: { device_id: DEVICE_ID } } }],
  ])("fails on %s", async (_, overrides: Partial<Record<string, Answer>>) => {
    const { binding, deps } = setup(fakeDaemon(answers(overrides)).open);
    const result = await pair(CODE, deps, { name: "iPhone" });
    expect(!result.ok && result.failure.kind).toBe("failed");
    expect(binding.keys.size).toBe(0);
  });

  it("fails when the connection closes while it waits for the terminal", async () => {
    const daemon = fakeDaemon(answers({ "device.enroll": "hang" }));
    const { binding, deps } = setup(daemon.open);
    const result = pair(CODE, deps, {
      name: "iPhone",
      onStep: (step) => {
        if (step.step === "confirm") {
          void later(() => daemon.sockets[0]?.close(1001, "efrd stops"));
        }
      },
    });
    expect(await result).toEqual({
      ok: false,
      failure: { kind: "failed", message: "the connection closed (code 1001): efrd stops" },
    });
    expect(binding.keys.size).toBe(0);
  });

  it("names a code that is not one this build can use, and opens no socket", async () => {
    const daemon = fakeDaemon(answers());
    const { deps } = setup(daemon.open);
    expect(await pair("https://example.com", deps, { name: "iPhone" })).toEqual({
      ok: false,
      failure: { kind: "not_efr" },
    });
    expect(await pair(CODE.replace("v=1", "v=2"), deps, { name: "iPhone" })).toEqual({
      ok: false,
      failure: { kind: "code_version" },
    });
    expect(await pair(CODE.replace("p=7878", "p=0"), deps, { name: "iPhone" })).toEqual({
      ok: false,
      failure: { kind: "malformed" },
    });
    expect(daemon.sockets.length).toBe(0);
  });
});

describe("cancellation", () => {
  it("calls each listener once, and at once after the cancel", () => {
    const { signal, cancel } = cancellation();
    const calls: string[] = [];
    const remove = signal.onCancel(() => calls.push("removed"));
    signal.onCancel(() => calls.push("first"));
    remove();
    expect(signal.cancelled).toBe(false);
    cancel();
    cancel();
    signal.onCancel(() => calls.push("late"));
    expect(signal.cancelled).toBe(true);
    expect(calls).toEqual(["first", "late"]);
  });
});
