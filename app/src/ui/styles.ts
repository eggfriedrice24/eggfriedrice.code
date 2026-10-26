// The shared styles of the screens. Each colour is a role of the design system
// (src/theme/palette.ts): surfaces, text and borders from `semantic`, states from its
// `state_*` roles, and the accent of buttons from `accent_primary`.
//
// Every style that a component passes is made here, once, so no render makes a new
// style object or array (the react-perf rules of the lint).

import { type PressableStateCallbackType, StyleSheet, type ViewStyle } from "react-native";

import { MONO_FONT } from "@/theme/markdown";
import { semantic } from "@/theme/palette";

import type { GaugeLevel } from "./format";

/** The space of the screen's edges, in points. */
export const GUTTER = 16;

/** The shared styles. */
export const common = StyleSheet.create({
  screen: { flex: 1, backgroundColor: semantic.surface_1 },
  center: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    padding: 24,
    gap: 12,
    backgroundColor: semantic.surface_1,
  },
  title: { color: semantic.text_bright, fontSize: 20, fontWeight: "600", textAlign: "center" },
  body: { color: semantic.text_primary, fontSize: 16, lineHeight: 22 },
  bodyCenter: { color: semantic.text_primary, fontSize: 16, lineHeight: 22, textAlign: "center" },
  muted: { color: semantic.text_muted, fontSize: 14, lineHeight: 20 },
  subtle: { color: semantic.text_subtle, fontSize: 13 },
  mono: { fontFamily: MONO_FONT, fontSize: 13, color: semantic.text_primary },
  button: {
    minHeight: 44,
    minWidth: 96,
    paddingHorizontal: 18,
    borderRadius: 10,
    alignItems: "center",
    justifyContent: "center",
    backgroundColor: semantic.accent_primary,
  },
  buttonText: { color: semantic.text_on_accent, fontSize: 16, fontWeight: "600" },
  buttonQuiet: {
    minHeight: 44,
    minWidth: 96,
    paddingHorizontal: 18,
    borderRadius: 10,
    alignItems: "center",
    justifyContent: "center",
    borderColor: semantic.border_default,
    borderWidth: 1,
  },
  buttonQuietText: { color: semantic.text_primary, fontSize: 16, fontWeight: "500" },
  link: { color: semantic.link, fontSize: 13 },
});

const PRESSED_OPACITY = 0.7;
const DISABLED_OPACITY = 0.4;

/** The style function of a Pressable on `base`: dimmed while pressed. */
export function pressable(base: ViewStyle): (state: PressableStateCallbackType) => ViewStyle {
  const pressed = { ...base, opacity: PRESSED_OPACITY };
  return (state) => (state.pressed ? pressed : base);
}

/** `base` dimmed, for a control that does not work now. */
export function dimmed(base: ViewStyle): ViewStyle {
  return { ...base, opacity: DISABLED_OPACITY };
}

/** The Pressable styles of the two buttons. */
export const BUTTON = pressable(common.button);
export const BUTTON_QUIET = pressable(common.buttonQuiet);
export const BUTTON_OFF = dimmed(common.button);
export const BUTTON_QUIET_OFF = dimmed(common.buttonQuiet);
/** A Pressable with no frame of its own, such as a text link. */
export const PLAIN = pressable({});

/** The colour of each level of the context gauge, as efr's terminal colours it. */
export const GAUGE = StyleSheet.create<Record<GaugeLevel, { color: string }>>({
  success: { color: semantic.state_success },
  warning: { color: semantic.state_warning },
  error: { color: semantic.state_error },
});

/** The text colour of each tone of a notice or a card. */
export const TONE = StyleSheet.create({
  muted: { color: semantic.text_muted },
  warning: { color: semantic.state_warning },
  success: { color: semantic.state_success },
  error: { color: semantic.state_error },
});

/** The accessibility states of a part that opens and closes. */
export const EXPANDED = { expanded: true } as const;
export const COLLAPSED = { expanded: false } as const;
