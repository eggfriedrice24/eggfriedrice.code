import { describe, expect, it } from "vitest";

import { flush } from "@/lib/net/testing";
import { FakePushNative } from "@/features/settings/model/testing";
import { createNoticeTaps } from "./notice-taps";

const DESKTOP = "019a9b1c-3d00-7a10-8b20-0000000000d1";
const UNKNOWN = "019a9b1c-3d00-7a10-8b20-0000000000d9";
const CONVERSATION = "019a9b1c-3d00-7a10-8b20-000000001001";

/** Desktop is the one machine that the phone knows. */
function knows(machine: string): Promise<boolean> {
  return Promise.resolve(machine === DESKTOP);
}

/** The taps on `source`, and the paths that they opened. */
function wire(source: FakePushNative) {
  const taps = createNoticeTaps();
  const paths: string[] = [];
  return { taps, paths, attach: () => taps.attach(source, knows) };
}

describe("the taps on a notice", () => {
  it("open the conversation of a known machine, once for each notice", async () => {
    const source = new FakePushNative();
    const { taps, paths, attach } = wire(source);
    attach();
    taps.subscribe((path) => paths.push(path));
    const tap = { id: "n1", data: { d: DESKTOP, c: CONVERSATION } };
    source.tap(tap);
    source.tap(tap);
    await flush();
    expect(paths).toEqual([`/c/${DESKTOP}/${CONVERSATION}`]);
  });

  it("drop a tap for a machine that the phone does not know, and data that is not a notice of efrd", async () => {
    const source = new FakePushNative();
    const { taps, paths, attach } = wire(source);
    attach();
    taps.subscribe((path) => paths.push(path));
    source.tap({ id: "n1", data: { d: UNKNOWN, c: CONVERSATION } });
    source.tap({ id: "n2", data: { d: DESKTOP, c: "not an id" } });
    source.tap({ id: "n3", data: undefined });
    await flush();
    expect(paths).toEqual([]);
  });

  it("open the tap that started the app when the router subscribes, also before the attach", async () => {
    const source = new FakePushNative();
    source.launchTap = { id: "n1", data: { d: DESKTOP, c: CONVERSATION } };
    const { taps, paths, attach } = wire(source);
    const stop = taps.subscribe((path) => paths.push(path));
    await flush();
    expect(paths).toEqual([]);
    attach();
    await flush();
    expect(paths).toEqual([`/c/${DESKTOP}/${CONVERSATION}`]);
    expect(source.cleared).toBe(1);

    // A second subscribe of the router does not open it again.
    stop();
    taps.subscribe((path) => paths.push(path));
    source.tap({ id: "n1", data: { d: DESKTOP, c: CONVERSATION } });
    await flush();
    expect(paths).toHaveLength(1);
  });
});
