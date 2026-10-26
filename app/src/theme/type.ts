// The type scale of the app, from the design canvas (design spec note 15 and section 3).
// scripts/gen-theme.mjs makes one uniwind class of each style: `monoXs` gives
// `text-mono-xs`, which sets the family, the size, the line height and the weight
// together. A component that cannot take a class (an Animated text, a native prop)
// spreads the style object instead. Text under 11 pt is not allowed (the test checks it).

/**
 * The monospace font of our own views. React Native maps `ui-monospace` to SF Mono with
 * Dynamic Type (UIFontDescriptorSystemDesignMonospaced in RCTFontUtils.mm of the new
 * architecture). Menus, the back history menu and action sheets are native and cannot
 * use it.
 */
export const MONO_FONT = "ui-monospace";

/** The weight of every bold style in the design (semibold). */
export const SEMIBOLD = "600";

/** One style of the type scale, in points. */
export interface TypeStyle {
  readonly fontSize: number;
  readonly lineHeight?: number;
  readonly fontWeight?: typeof SEMIBOLD;
  readonly fontFamily?: typeof MONO_FONT;
}

/** The smallest text size of the app, in points. */
export const MIN_TEXT_SIZE = 11;

const mono = { fontFamily: MONO_FONT } as const;
const semibold = { fontWeight: SEMIBOLD } as const;

/** The type scale. The system font (SF Pro) is the default; `mono*` styles use SF Mono. */
export const TYPE = {
  // SF Pro.
  /** Prose, your messages and the pairing sentences (Subheadline based). */
  body: { fontSize: 14, lineHeight: 21 },
  /** The composer input (Subheadline based). */
  input: { fontSize: 15, lineHeight: 21 },
  /** Titles and values of Settings rows, titles of sheet rows and the search text. */
  row: { fontSize: 15, lineHeight: 20 },
  /** The header title of a conversation. */
  title: { fontSize: 15, lineHeight: 20, ...semibold },
  /** The approval reason and the text of the forget action sheet. */
  reason: { fontSize: 13, lineHeight: 18 },
  /** The reasoning text under an open thought row. */
  thought: { fontSize: 13, lineHeight: 19 },
  /** Group headers and footers of Settings. */
  caption: { fontSize: 12, lineHeight: 16 },
  /** Markdown tables. */
  table: { fontSize: 12.5, lineHeight: 17 },
  /** Markdown h1 (Headline based). */
  heading1: { fontSize: 18, lineHeight: 24, ...semibold },
  /** Markdown h2. */
  heading2: { fontSize: 16, lineHeight: 22, ...semibold },
  /** Markdown h3 to h6. */
  heading3: { fontSize: 15, lineHeight: 20, ...semibold },
  /** The titles of the pairing screens (Title 3). */
  pairingTitle: { fontSize: 20, lineHeight: 25, ...semibold },
  /** Allow and Deny. */
  button: { fontSize: 15, ...semibold },
  /** The primary and the bordered buttons of the pairing screens. */
  buttonLarge: { fontSize: 16, ...semibold },
  /** The Steer and Queue capsules of the composer. */
  capsule: { fontSize: 14, ...semibold },

  // SF Mono.
  /** The minimum: tool and approval headers, output, the status line, hints and tags. */
  monoXs: { fontSize: 11, lineHeight: 16, ...mono },
  /** The turn end line and the section headers of the sheet. */
  monoXsTight: { fontSize: 11, lineHeight: 15, ...mono },
  /** Diff lines. */
  monoDiff: { fontSize: 11, lineHeight: 17, ...mono },
  /** "✓ paired", the line above a pairing error and the scanner hint. */
  monoSm: { fontSize: 12, lineHeight: 17, ...mono },
  /** Code block bodies, and inline mono inside the approval reason. */
  monoCode: { fontSize: 12, lineHeight: 18, ...mono },
  /** Inline code in Markdown. It takes the line height of its paragraph. */
  monoInline: { fontSize: 12.5, ...mono },
  /** The command of an approval block. */
  monoCommand: { fontSize: 12.5, lineHeight: 18, ...mono },
  /** The shell block, the node name of pairing success and the version in Settings. */
  monoShell: { fontSize: 13, lineHeight: 19, ...mono },
  /** The machine name of a Settings row. */
  monoName: { fontSize: 14, lineHeight: 19, ...mono },
  /** The command box of the pairing intro. */
  monoLarge: { fontSize: 15, lineHeight: 20, ...mono },
  /** The values of a key and value box (the device and the key of pairing confirm). */
  monoValue: { fontSize: 16, lineHeight: 22, ...mono },
  /** The header title of the blank root (the machine name). */
  monoTitle: { fontSize: 15, lineHeight: 20, ...semibold, ...mono },
  /** A machine name inside a pairing title. */
  monoHeading: { fontSize: 19, lineHeight: 25, ...semibold, ...mono },
  /** The machine name of pairing success. */
  monoDisplay: { fontSize: 26, lineHeight: 32, ...semibold, ...mono },
} as const satisfies Record<string, TypeStyle>;

/** The name of a style of the type scale. */
export type TypeName = keyof typeof TYPE;

/** The letter spacing of the key fingerprint in the pairing confirm box, in points. */
export const FINGERPRINT_LETTER_SPACING = 0.5;
