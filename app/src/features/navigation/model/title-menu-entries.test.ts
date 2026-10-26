import { describe, expect, it } from "vitest";

import { menuActions } from "@/components/ui/menu-actions";
import type { TitleMenuRow } from "@/features/conversations/model/title-menu";
import { roles } from "@/theme/roles";
import {
  ALL_CONVERSATIONS,
  titleLabel,
  titleMenuAction,
  titleMenuEntries,
  titleMenuTitle,
} from "./title-menu-entries";

const DESKTOP = "019a9b1c-3d00-7a10-8b20-0000000000d1";
const ID = "019a9b1c-3d00-7a10-8b20-000000001001";

function row(overrides: Partial<TitleMenuRow>): TitleMenuRow {
  return {
    machine: DESKTOP,
    id: ID,
    title: "Fix steer race in turn loop",
    subtitle: "~/code/efr · 2m",
    glyph: "none",
    current: false,
    ...overrides,
  };
}

describe("titleMenuEntries", () => {
  it("gives a waiting row the yolk glyph, which keeps its colour", () => {
    const [action] = menuActions(
      titleMenuEntries([row({ glyph: "approval", subtitle: "Needs approval · ~/code/efr" })]),
    );
    expect(action).toMatchObject({
      id: `open:${DESKTOP}:${ID}`,
      title: "Fix steer race in turn loop",
      subtitle: "Needs approval · ~/code/efr",
      image: "exclamationmark.circle.fill",
      imageColor: roles.accent_primary,
    });
  });

  it("gives an idle row no image, and the current row the check", () => {
    const [action] = menuActions(titleMenuEntries([row({ current: true })]));
    expect(action).not.toHaveProperty("image");
    expect(action).toMatchObject({ state: "on" });
  });

  it("ends with All conversations in its own section", () => {
    const entries = menuActions(titleMenuEntries([]));
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      displayInline: true,
      subactions: [
        { id: ALL_CONVERSATIONS, title: "All conversations…", image: "magnifyingglass" },
      ],
    });
  });
});

describe("titleMenuAction", () => {
  it("reads the conversation of a row", () => {
    expect(titleMenuAction(`open:${DESKTOP}:${ID}`)).toEqual({
      kind: "open",
      conversation: { machine: DESKTOP, id: ID },
    });
  });

  it("reads All conversations, and nothing for an id that the menu did not make", () => {
    expect(titleMenuAction(ALL_CONVERSATIONS)).toEqual({ kind: "all" });
    expect(titleMenuAction("open:")).toBeUndefined();
    expect(titleMenuAction(`open:${DESKTOP}:`)).toBeUndefined();
    expect(titleMenuAction("other")).toBeUndefined();
  });
});

describe("the words of the title", () => {
  it("names the menu after the machine", () => {
    expect(titleMenuTitle("desktop")).toBe("Recent on desktop");
  });

  it("adds the approvals that wait to the label", () => {
    expect(titleLabel("desktop", 0)).toBe("desktop, recent conversations");
    expect(titleLabel("Fix steer race", 1)).toBe(
      "Fix steer race, recent conversations, 1 approval waiting",
    );
    expect(titleLabel("Fix steer race", 3)).toBe(
      "Fix steer race, recent conversations, 3 approvals waiting",
    );
  });
});
