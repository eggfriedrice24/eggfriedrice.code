import { describe, expect, it } from "vitest";

import { roles } from "@/theme/roles";

import { menuActions, type MenuEntry } from "./menu-actions";

describe("the actions of a menu", () => {
  it("never send an image key for an item without a symbol", () => {
    const actions = menuActions([
      { id: "a", title: "Expo SDK 58 upgrade" },
      { id: "b", title: "Fix steer race", image: "" },
      { id: "c", title: "Why is the fan loud", image: "", imageTone: "accent_primary" },
    ]);
    for (const action of actions) {
      expect("image" in action).toBe(false);
      expect("imageColor" in action).toBe(false);
    }
  });

  it("keep the colour of a glyph that sets a tone", () => {
    const [action] = menuActions([
      {
        id: "x",
        title: "Fix steer race in turn loop",
        subtitle: "Needs approval · ~/code/efr",
        image: "exclamationmark.circle.fill",
        imageTone: "accent_primary",
      },
    ]);
    expect(action).toEqual({
      id: "x",
      title: "Fix steer race in turn loop",
      subtitle: "Needs approval · ~/code/efr",
      image: "exclamationmark.circle.fill",
      imageColor: roles.accent_primary,
    });
  });

  it("give every image a colour, because Fabric turns a missing one into clear", () => {
    const [action] = menuActions([{ id: "d", title: "desktop", image: "desktopcomputer" }]);
    expect(action).toEqual({
      id: "d",
      title: "desktop",
      image: "desktopcomputer",
      imageColor: roles.text_bright,
    });
  });

  it("send the check, the destructive and the disabled attributes", () => {
    const actions = menuActions([
      { id: "on", title: "desktop", subtitle: "Connected", state: "on" },
      { id: "forget", title: "Forget this machine", image: "trash", destructive: true },
      { id: "off", title: "laptop", disabled: true },
    ]);
    expect(actions).toEqual([
      { id: "on", title: "desktop", subtitle: "Connected", state: "on" },
      {
        id: "forget",
        title: "Forget this machine",
        image: "trash",
        imageColor: roles.state_error,
        attributes: { destructive: true },
      },
      { id: "off", title: "laptop", attributes: { disabled: true } },
    ]);
  });

  it("put a section inline, after a gap", () => {
    const entries: MenuEntry[] = [
      { id: "desktop", title: "desktop", state: "on" },
      {
        section: "pair",
        items: [{ id: "pair", title: "Pair a new machine", image: "qrcode.viewfinder" }],
      },
    ];
    expect(menuActions(entries)).toEqual([
      { id: "desktop", title: "desktop", state: "on" },
      {
        id: "pair",
        title: "",
        displayInline: true,
        subactions: [
          {
            id: "pair",
            title: "Pair a new machine",
            image: "qrcode.viewfinder",
            imageColor: roles.text_bright,
          },
        ],
      },
    ]);
  });

  it("drop an empty subtitle", () => {
    const [action] = menuActions([{ id: "a", title: "a", subtitle: "" }]);
    expect(action).toEqual({ id: "a", title: "a" });
  });
});
