// The links from outside on their way to the router (src/app/+native-intent.ts): a link
// that starts the app goes at once, and a later link waits in the gate of the links until
// the Face ID unlock shows the app. The test lives outside src/app, which holds routes.
import { describe, expect, it } from "vitest";

import { redirectSystemPath } from "@/app/+native-intent";
import { linkGate } from "@/features/navigation/model/link-gate";
import { flush } from "@/lib/net/testing";

describe("redirectSystemPath", () => {
  it("lets the link that starts the app go at once", () => {
    expect(redirectSystemPath({ path: "/conversations", initial: true })).toBe("/conversations");
  });

  it("holds a later link until the lock shows the app", async () => {
    let show: (() => void) | undefined;
    const shown = new Promise<void>((resolve) => {
      show = resolve;
    });
    const passed: string[] = [];
    const later = redirectSystemPath({ path: "/pair", initial: false });
    expect(typeof later).not.toBe("string");
    void Promise.resolve(later).then((path) => passed.push(path));
    linkGate.follow({ whenShown: () => shown });
    await flush();
    expect(passed).toEqual([]);
    show?.();
    await flush();
    expect(passed).toEqual(["/pair"]);
  });
});
