// The push service on the real services of the app, with fake sockets and a fake native
// side of expo-notifications. No test reaches Expo or Apple.

import { afterEach, describe, expect, it } from "vitest";

import { flush, type FakeSocket } from "@/lib/net/testing";
import type { HelloResult } from "@/lib/protocol/generated";
import {
  DESKTOP,
  LAPTOP,
  lastSocket,
  testServices,
  type TestServices,
} from "@/features/navigation/model/testing";
import { machineRecord } from "@/features/machines/model/testing";
import type { Push } from "./push";
import { FakePushNative, expoToken } from "./testing";

/** The newest socket of `machine`. */
function socketOf(parts: TestServices, machine: string): FakeSocket {
  const url = machine === DESKTOP ? "ws://100.64.0.1:7419/" : "ws://100.64.0.2:7419/";
  return lastSocket(parts.sockets, url);
}

/** The hello of an efrd that takes a push token. */
const PUSH_HELLO: Partial<HelloResult> = {
  capabilities: { drafts_coalesced: true, device_prove: true, push: true },
};

let opened: TestServices[] = [];

async function setup(options: { readonly on?: boolean } = {}) {
  const native = new FakePushNative();
  if (options.on === true) {
    native.permissionNow = "granted";
  }
  const parts = await testServices([machineRecord(DESKTOP), machineRecord(LAPTOP)], {
    push: native,
    notificationsOn: options.on === true,
  });
  opened.push(parts);
  await flush();
  const push: Push | null = parts.services.push;
  if (push === null) {
    throw new Error("the services made no push service");
  }
  return { parts, native, push };
}

/** The params of each `device.push_set` that the phone sent to `machine`. */
function tokens(parts: TestServices, machine: string): unknown[] {
  return socketOf(parts, machine)
    .requests("device.push_set")
    .map((frame) => frame["params"]);
}

afterEach(() => {
  for (const parts of opened) {
    parts.services.dispose();
  }
  opened = [];
});

describe("the switch Notifications", () => {
  it("is off by default, and a machine that takes a token gets none at each connection", async () => {
    const { parts, native, push } = await setup();
    expect(push.store.get()).toMatchObject({ on: false, permission: "undetermined" });
    await parts.connect(DESKTOP, PUSH_HELLO);
    expect(tokens(parts, DESKTOP)).toEqual([{}]);
    expect(native.questions).toBe(0);
    expect(native.tokenRequests).toBe(0);
  });

  it("lets iOS ask on the first tap, then gives the token to each machine that takes one", async () => {
    const { parts, native, push } = await setup();
    await parts.connect(DESKTOP, PUSH_HELLO);
    await parts.connect(LAPTOP);
    push.setOn(true);
    expect(push.store.get().asking).toBe(true);
    await flush();
    expect(native.questions).toBe(1);
    expect(push.store.get()).toMatchObject({ on: true, permission: "granted", asking: false });
    expect(parts.services.preferences.store.get().notifications).toBe(true);
    expect(tokens(parts, DESKTOP)).toEqual([{}, { token: expoToken(1) }]);
    // An efrd without the capability gets nothing, and Settings names it.
    expect(tokens(parts, LAPTOP)).toEqual([]);
    expect(push.store.get().unavailable).toEqual([LAPTOP]);
  });

  it("stays off when iOS denies, and sends no token", async () => {
    const { parts, native, push } = await setup();
    native.answer = "denied";
    await parts.connect(DESKTOP, PUSH_HELLO);
    push.setOn(true);
    await flush();
    expect(push.store.get()).toMatchObject({ on: false, permission: "denied" });
    expect(parts.services.preferences.store.get().notifications).toBe(false);
    expect(native.tokenRequests).toBe(0);
    expect(tokens(parts, DESKTOP)).toEqual([{}, {}]);
  });

  it("sends the token again at each connection while it is on", async () => {
    const { parts, native } = await setup({ on: true });
    await parts.connect(DESKTOP, PUSH_HELLO);
    expect(tokens(parts, DESKTOP)).toEqual([{ token: expoToken(1) }]);
    socketOf(parts, DESKTOP).drop();
    await flush();
    await parts.clock.advance(1000);
    await parts.connect(DESKTOP, PUSH_HELLO);
    expect(tokens(parts, DESKTOP)).toEqual([{ token: expoToken(1) }]);
    // The app asks Expo once: the token stays in memory.
    expect(native.tokenRequests).toBe(1);
  });

  it("tries the request to Expo again at the next connection after it failed", async () => {
    const { parts, native } = await setup({ on: true });
    native.nextToken = undefined;
    await parts.connect(DESKTOP, PUSH_HELLO);
    expect(tokens(parts, DESKTOP)).toEqual([]);
    expect(parts.errors).toHaveLength(1);
    native.nextToken = expoToken(1);
    socketOf(parts, DESKTOP).drop();
    await flush();
    await parts.clock.advance(1000);
    await parts.connect(DESKTOP, PUSH_HELLO);
    expect(tokens(parts, DESKTOP)).toEqual([{ token: expoToken(1) }]);
  });

  it("gives each machine the new Expo token after a new APNs token", async () => {
    const { parts, native } = await setup({ on: true });
    await parts.connect(DESKTOP, PUSH_HELLO);
    native.nextToken = expoToken(2);
    native.newApnsToken();
    await flush();
    expect(tokens(parts, DESKTOP)).toEqual([{ token: expoToken(1) }, { token: expoToken(2) }]);
  });

  it("sends no token when it goes off, and before the app forgets a machine", async () => {
    const { parts, push } = await setup({ on: true });
    await parts.connect(DESKTOP, PUSH_HELLO);
    push.setOn(false);
    expect(parts.services.preferences.store.get().notifications).toBe(false);
    push.release(DESKTOP);
    await flush();
    expect(tokens(parts, DESKTOP)).toEqual([{ token: expoToken(1) }, {}, {}]);
  });

  it("reads the permission again at each return to the foreground", async () => {
    const { parts, native, push } = await setup({ on: true });
    await parts.connect(DESKTOP, PUSH_HELLO);
    native.permissionNow = "denied";
    parts.setActive(false);
    parts.setActive(true);
    await flush();
    expect(push.store.get().permission).toBe("denied");
    expect(tokens(parts, DESKTOP)).toEqual([{ token: expoToken(1) }, {}]);
  });

  it("removes the delivered notices of a conversation on screen", async () => {
    const { native, push } = await setup();
    push.shown("019a9b1c-3d00-7a10-8b20-000000001001");
    expect(native.dismissed).toEqual(["019a9b1c-3d00-7a10-8b20-000000001001"]);
  });
});
