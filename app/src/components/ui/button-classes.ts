// The sizes and states of the filled and the bordered buttons, shared by
// primary-button.tsx and bordered-button.tsx.
//
// - `large`: the buttons of the pairing screens, 50 pt high, radius 12, 16 semibold.
// - `medium`: Allow and Deny of an approval block, 44 pt high, radius 10, 15 semibold.
//
// A disabled button is at 40% (Deny while the answer is sent, design 3.11).

import { TYPE_CLASS } from "./text-classes";

/** The size of a button. */
export type ButtonSize = "large" | "medium";

/** The frame of each size. */
export const BUTTON_FRAME = {
  large: "h-button rounded-control",
  medium: "h-hit rounded-box",
} as const satisfies Record<ButtonSize, string>;

/** The label style of each size. */
export const BUTTON_LABEL = {
  large: TYPE_CLASS.buttonLarge,
  medium: TYPE_CLASS.button,
} as const satisfies Record<ButtonSize, string>;

/** The classes that every button has: a centred row with an 8 pt gap. */
export const BUTTON_BASE = "flex-row items-center justify-center gap-8 px-16";

/** A disabled button (OPACITY.pending of theme/materials.ts). */
export const BUTTON_DISABLED = "opacity-40";

/** A button that takes its share of a row, as Allow and Deny do. */
export const BUTTON_GROW = "flex-1";
