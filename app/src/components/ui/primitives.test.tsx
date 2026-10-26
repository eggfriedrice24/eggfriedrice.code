import { describe, expect, it, jest } from "@jest/globals";
import { render, screen } from "@testing-library/react-native";
import { StyleSheet, View } from "react-native";

import { MATERIALS } from "@/theme/materials";
import { roles } from "@/theme/roles";

import { BodyText } from "./body-text";
import { Dot } from "./dot";
import { Hairline } from "./hairline";
import { LIQUID_GLASS, Material } from "./material";
import { MonoText } from "./mono-text";
import { SectionHeader, sectionHeaderText } from "./section-header";
import { SymbolIcon } from "./symbol";

const HIDDEN = { includeHiddenElements: true } as const;

describe("the text primitives", () => {
  it("give mono text the minimum size and the quiet colour by default", async () => {
    await render(<MonoText>in ~/code/efr</MonoText>);
    expect(screen.getByText("in ~/code/efr").props.className).toBe("text-mono-xs text-text-subtle");
  });

  it("give body text the prose size and text_primary by default", async () => {
    await render(<BodyText>Run this in a terminal.</BodyText>);
    expect(screen.getByText("Run this in a terminal.").props.className).toBe(
      "text-body text-text-primary",
    );
  });

  it("take a size, a tone and more classes", async () => {
    await render(
      <MonoText size="monoShell" tone="text_bright" className="flex-1" numberOfLines={1}>
        desktop
      </MonoText>,
    );
    const text = screen.getByText("desktop");
    expect(text.props.className).toBe("text-mono-shell text-text-bright flex-1");
    expect(text.props.numberOfLines).toBe(1);
  });
});

describe("the dot", () => {
  it("has a label when it is the only mark", async () => {
    await render(<Dot size="large" label="Needs approval" />);
    const dot = screen.getByLabelText("Needs approval");
    expect(dot.props.className).toContain("w-8 h-8");
    expect(dot.props.className).toContain("bg-accent-primary");
  });

  it("is hidden from VoiceOver next to words", async () => {
    await render(<Dot size="small" />);
    expect(screen.queryByTestId("dot")).toBeNull();
    expect(screen.getByTestId("dot", HIDDEN).props.className).toContain("w-7 h-7");
  });
});

describe("the hairline", () => {
  it("is 0.5 pt of border_subtle, and can start 16 or 20 pt in", async () => {
    await render(
      <View>
        <Hairline />
        <Hairline inset="inset" />
        <Hairline inset="gutter" grow />
      </View>,
    );
    const [full, inset, gutter] = screen.getAllByTestId("hairline", HIDDEN);
    expect(full?.props.className).toBe("h-hairline bg-border-subtle");
    expect(inset?.props.className).toBe("h-hairline bg-border-subtle ml-inset");
    expect(gutter?.props.className).toBe("h-hairline bg-border-subtle ml-gutter flex-1");
  });
});

describe("the symbol", () => {
  it("is decoration without a label", async () => {
    await render(<SymbolIcon name="chevron.down" size={11} tone="text_muted" />);
    expect(screen.queryByRole("image")).toBeNull();
  });

  it("has the label it is given", async () => {
    await render(<SymbolIcon name="clock" size={12} tone="text_subtle" label="Expired" />);
    expect(screen.getByRole("image", { name: "Expired" })).toBeOnTheScreen();
  });
});

describe("the section header", () => {
  it("shows the count of the waiting section", () => {
    expect(sectionHeaderText("waiting", 2)).toBe("waiting · 2");
    expect(sectionHeaderText("today", undefined)).toBe("today");
  });

  it("is a header in yolk for the waiting section", async () => {
    await render(<SectionHeader title="waiting" count={2} waiting />);
    const header = screen.getByRole("header", { name: "waiting · 2" });
    expect(header.props.className).toContain("text-accent-primary");
    expect(header.props.className).toContain("text-mono-xs-tight");
  });

  it("is quiet for the other sections, with a hairline under it", async () => {
    await render(<SectionHeader title="this week" />);
    expect(screen.getByRole("header", { name: "this week" }).props.className).toContain(
      "text-text-subtle",
    );
    expect(screen.getByTestId("hairline", HIDDEN)).toBeOnTheScreen();
  });
});

describe("the material", () => {
  it("is the chrome blur of the bar", async () => {
    await render(<Material kind="chrome" />);
    expect(screen.getByTestId("material-chrome", HIDDEN)).toBeOnTheScreen();
    const tints = screen.container.queryAll((node) => node.props.tint !== undefined);
    expect(tints.every((node) => node.props.tint === "systemChromeMaterialDark")).toBe(true);
    expect(tints.length).toBeGreaterThan(0);
  });

  it("is Liquid Glass where iOS has it", async () => {
    expect(LIQUID_GLASS).toBe(true);
    await render(<Material kind="glass" radius={22} />);
    const glass = screen.getByTestId("material-glass", HIDDEN);
    expect(StyleSheet.flatten(glass.props.style)).toMatchObject({ borderRadius: 22 });
  });

  it("is the blur with the canvas fill and edge before iOS 26", async () => {
    jest.resetModules();
    jest.doMock("expo-glass-effect", () => ({
      GlassView: () => null,
      isLiquidGlassAvailable: () => false,
    }));
    const fallback: typeof import("./material") = require("./material");
    expect(fallback.LIQUID_GLASS).toBe(false);
    await render(<fallback.Material kind="glass" radius={22} />);
    expect(screen.getByTestId("material-glass", HIDDEN)).toBeOnTheScreen();
    const edges = screen.container.queryAll(
      (node) => StyleSheet.flatten(node.props.style)?.borderColor === roles.border_subtle,
    );
    expect(edges).toHaveLength(1);
    expect(StyleSheet.flatten(edges[0]?.props.style)).toMatchObject({
      borderWidth: 1,
      borderRadius: 22,
      backgroundColor: MATERIALS.glassButton.fill,
    });
    jest.dontMock("expo-glass-effect");
  });
});
