import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { act, render, screen } from "@testing-library/react-native";
import { AccessibilityInfo, StyleSheet } from "react-native";

import { roles } from "@/theme/roles";

import { BlockCursor } from "./block-cursor";
import { BrailleCell } from "./braille-cell";
import { BRAILLE_STILL } from "./loops";

const HIDDEN = { includeHiddenElements: true } as const;

/** Sets what iOS answers for Reduce Motion, before a render. */
function reduceMotion(on: boolean): void {
  jest.spyOn(AccessibilityInfo, "isReduceMotionEnabled").mockResolvedValue(on);
}

/** Lets the Reduce Motion promise resolve. */
async function settle(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

function styleOf(testID: string, index = 0) {
  const element = screen.getAllByTestId(testID, HIDDEN)[index];
  return StyleSheet.flatten(element?.props.style);
}

function dotOpacities(): unknown[] {
  return screen
    .getAllByTestId("braille-dot", HIDDEN)
    .map((dot) => StyleSheet.flatten(dot.props.style).opacity);
}

beforeEach(() => {
  reduceMotion(false);
});

afterEach(() => {
  jest.restoreAllMocks();
});

describe("the braille cell", () => {
  it("has the label of what runs, as an image", async () => {
    await render(<BrailleCell label="Running" />);
    const cell = screen.getByLabelText("Running");
    expect(cell).toBeOnTheScreen();
    expect(cell.props.accessibilityRole).toBe("image");
  });

  it("is hidden from VoiceOver without a label", async () => {
    await render(<BrailleCell />);
    expect(screen.queryByTestId("braille-cell")).toBeNull();
    expect(screen.getByTestId("braille-cell", HIDDEN)).toBeOnTheScreen();
  });

  it("draws 8 dots in the size of its class", async () => {
    await render(<BrailleCell size="row" />);
    expect(screen.getAllByTestId("braille-dot", HIDDEN)).toHaveLength(8);
    expect(screen.getByTestId("braille-cell", HIDDEN).props.className).toContain("w-8 h-15");
  });

  it("tints the dots by who works", async () => {
    await render(<BrailleCell tint="wait" />);
    expect(styleOf("braille-dot").backgroundColor).toBe(roles.text_muted);
    await render(<BrailleCell tint="on_accent" />);
    expect(styleOf("braille-dot").backgroundColor).toBe(roles.text_on_accent);
    await render(<BrailleCell />);
    expect(styleOf("braille-dot").backgroundColor).toBe(roles.accent_primary);
  });

  it("runs the loop: the first step lights dot 0 with the trail behind it", async () => {
    await render(<BrailleCell />);
    await settle();
    expect(dotOpacities()).toEqual([1, 0.16, 0.16, 0.16, 0.16, 0.16, 0.4, 0.7]);
  });

  it("shows the still frame with Reduce Motion", async () => {
    reduceMotion(true);
    await render(<BrailleCell />);
    await settle();
    expect(dotOpacities()).toEqual(BRAILLE_STILL);
  });

  it("shows the still frame when the caller asks for it", async () => {
    await render(<BrailleCell still />);
    expect(dotOpacities()).toEqual(BRAILLE_STILL);
  });

  it("follows a change of Reduce Motion while it is on screen", async () => {
    const listen = jest
      .spyOn(AccessibilityInfo, "addEventListener")
      .mockReturnValue({ remove: () => undefined });
    // The spy is the mock of the React Native preset: forget the calls of the tests before.
    listen.mockClear();
    await render(<BrailleCell />);
    await settle();
    expect(dotOpacities()[1]).toBe(0.16);
    await act(async () => {
      listen.mock.calls[0]?.[1](true);
    });
    expect(dotOpacities()).toEqual(BRAILLE_STILL);
  });
});

describe("the block cursor", () => {
  it("is yolk and has the size of the shell line (design item 8.1)", async () => {
    await render(<BlockCursor size="shell" mode="solid" />);
    expect(styleOf("block-cursor")).toMatchObject({
      width: 9,
      height: 19,
      backgroundColor: roles.accent_primary,
      opacity: 1,
    });
  });

  it("has the sizes of prose and code", async () => {
    await render(<BlockCursor size="prose" mode="solid" />);
    expect(styleOf("block-cursor")).toMatchObject({ width: 7, height: 14 });
    await render(<BlockCursor size="code" mode="solid" />);
    expect(styleOf("block-cursor")).toMatchObject({ width: 6, height: 13 });
  });

  it("stays at 35% on the no-machine root", async () => {
    await render(<BlockCursor size="shell" mode="idle" />);
    expect(styleOf("block-cursor").opacity).toBe(0.35);
  });

  it("starts on when it blinks", async () => {
    await render(<BlockCursor size="shell" mode="blink" />);
    await settle();
    expect(styleOf("block-cursor").opacity).toBe(1);
  });

  it("is solid with Reduce Motion", async () => {
    reduceMotion(true);
    await render(<BlockCursor size="prose" mode="streaming" activity={3} />);
    await settle();
    expect(styleOf("block-cursor").opacity).toBe(1);
  });

  it("is on while tokens arrive", async () => {
    const { rerender } = await render(<BlockCursor size="prose" mode="streaming" activity={1} />);
    await settle();
    expect(styleOf("block-cursor").opacity).toBe(1);
    await rerender(<BlockCursor size="prose" mode="streaming" activity={2} />);
    expect(styleOf("block-cursor").opacity).toBe(1);
  });

  it("is hidden from VoiceOver", async () => {
    await render(<BlockCursor size="shell" mode="blink" />);
    expect(screen.queryByTestId("block-cursor")).toBeNull();
  });
});
