// The materials of the design: the blur of the bar, the strip and the composer, the
// glass buttons, the jump button and the dim layer. The real app uses the system blur
// (design note 10). The canvas draws each material as a role at an opacity, and that
// fill is what shows before iOS 26 glass and with Reduce Transparency.

import { BORDER } from "./metrics";
import { roles, withOpacity } from "./roles";

/** The system material of the bar, the strip and the composer (a tint of expo-blur). */
export const CHROME_BLUR_TINT = "systemChromeMaterialDark";

/** The materials of the app. */
export const MATERIALS = {
  /** The bar, the strip under it and the docked composer. */
  chrome: {
    blurTint: CHROME_BLUR_TINT,
    /** The full strength of the system material, as the native header blur has it. */
    blurIntensity: 100,
    fill: withOpacity("surface_1", 0.76),
  },
  /** A glass button or group of the header before iOS 26 glass. */
  glassButton: {
    fill: withOpacity("surface_2", 0.66),
    borderColor: roles.border_subtle,
    borderWidth: BORDER,
  },
  /** The "Jump to latest" button. */
  jump: {
    fill: withOpacity("surface_2", 0.7),
    borderColor: roles.border_subtle,
    borderWidth: BORDER,
    shadow: {
      color: roles.surface_0,
      opacity: 0.6,
      radius: 18,
      offsetY: 6,
    },
  },
  /** The dim layer behind an action sheet in the canvas. The real app uses the system dim. */
  dim: withOpacity("surface_0", 0.5),
} as const;

/** The opacities of states in the design. */
export const OPACITY = {
  /** A disconnected composer: its input cannot take text. */
  disconnected: 0.6,
  /** Deny while the answer is sent. */
  pending: 0.4,
  /** The cursor of the no-machine root, which never blinks. */
  idleCursor: 0.35,
  /** The low step of a streaming cursor that blinks after a silence. */
  cursorBlinkLow: 0.25,
} as const;
