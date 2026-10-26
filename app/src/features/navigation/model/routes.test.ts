import { describe, expect, it } from "vitest";

import {
  conversationHref,
  conversationOfParams,
  conversationsHref,
  conversationsSheetHref,
  entersInstantly,
  pairHref,
  stackRouteOf,
} from "./routes";
import { headerOptions, screenOptions } from "./screen-options";

const MACHINE = "019a9b1c-3d00-7a10-8b20-0000000000d1";
const ID = "019a9b1c-3d00-7a10-8b20-000000001001";

describe("the links", () => {
  it("links to a conversation with its machine and id", () => {
    expect(conversationHref({ machine: MACHINE, id: ID })).toEqual({
      pathname: "/c/[machine]/[id]",
      params: { machine: MACHINE, id: ID },
    });
  });

  it("marks the first send and the focus on the approval", () => {
    expect(
      conversationHref({ machine: MACHINE, id: ID }, { firstSend: true, focus: "approval" }),
    ).toEqual({
      pathname: "/c/[machine]/[id]",
      params: { machine: MACHINE, id: ID, enter: "instant", focus: "approval" },
    });
  });

  it("links to the sheet, with the waiting section on top when asked", () => {
    expect(conversationsHref()).toBe("/conversations");
    expect(conversationsSheetHref(MACHINE)).toEqual({
      pathname: "/conversations",
      params: { machine: MACHINE },
    });
    expect(conversationsHref("waiting")).toEqual({
      pathname: "/conversations",
      params: { section: "waiting" },
    });
  });

  it("links to Pairing and to the re-pair of a machine", () => {
    expect(pairHref()).toBe("/pair");
    expect(pairHref(MACHINE)).toEqual({ pathname: "/pair", params: { machine: MACHINE } });
  });
});

describe("the params of a conversation route", () => {
  it("reads the machine and the id", () => {
    expect(conversationOfParams({ machine: MACHINE, id: ID, enter: "instant" })).toEqual({
      machine: MACHINE,
      id: ID,
    });
  });

  it("refuses params that are not UUIDs", () => {
    expect(conversationOfParams({ machine: "desktop", id: ID })).toBeUndefined();
    expect(conversationOfParams({ machine: MACHINE, id: "" })).toBeUndefined();
    expect(conversationOfParams(undefined)).toBeUndefined();
  });

  it("refuses an unknown focus", () => {
    expect(conversationOfParams({ machine: MACHINE, id: ID, focus: "tool" })).toBeUndefined();
  });
});

describe("stackRouteOf", () => {
  it("maps the routes of the root stack", () => {
    expect(stackRouteOf({ name: "index" })).toEqual({ name: "index" });
    expect(
      stackRouteOf({ name: "c/[machine]/[id]", params: { machine: MACHINE, id: ID } }),
    ).toEqual({ name: "c", machine: MACHINE, id: ID });
    expect(stackRouteOf({ name: "settings" })).toEqual({ name: "other" });
    expect(stackRouteOf({ name: "conversations" })).toEqual({ name: "other" });
  });

  it("takes a conversation route with bad params as another screen", () => {
    expect(stackRouteOf({ name: "c/[machine]/[id]", params: { machine: "x" } })).toEqual({
      name: "other",
    });
  });
});

describe("the screen options", () => {
  it("pushes the first send without an animation", () => {
    expect(entersInstantly({ enter: "instant" })).toBe(true);
    expect(screenOptions({ enter: "instant" }, true).animation).toBe("none");
    expect(screenOptions({ machine: MACHINE }, true).animation).toBe("default");
    expect(screenOptions(undefined, false).animation).toBe("default");
  });

  it("gives every bar the transparent native look", () => {
    for (const liquidGlass of [true, false]) {
      expect(screenOptions(undefined, liquidGlass)).toMatchObject({
        headerTransparent: true,
        headerBackButtonDisplayMode: "minimal",
        headerBackButtonMenuEnabled: true,
        headerLargeTitle: false,
      });
    }
  });

  it("draws no bar material on iOS 26: only the soft scroll edge at the top", () => {
    const options = headerOptions(true);
    expect(options.headerBlurEffect).toBeUndefined();
    expect(options.headerStyle).toEqual({ backgroundColor: "transparent" });
    expect(options.headerShadowVisible).toBe(false);
    expect(options.scrollEdgeEffects).toEqual({
      top: "automatic",
      bottom: "hidden",
      left: "hidden",
      right: "hidden",
    });
  });

  it("keeps the chrome blur of the bar before iOS 26, with every edge hidden", () => {
    const options = headerOptions(false);
    expect(options.headerBlurEffect).toBe("systemChromeMaterialDark");
    expect(options.scrollEdgeEffects).toEqual({
      top: "hidden",
      bottom: "hidden",
      left: "hidden",
      right: "hidden",
    });
  });
});
