// The measurements of the design canvas, in points. scripts/gen-theme.mjs makes uniwind
// tokens of SPACING and RADII: `gutter` gives `px-gutter`, `gap-gutter` and so on, and
// `box` gives `rounded-box`. The theme sets the spacing unit to 1 pt, so any other value
// of the design is a plain class with its number (`gap-6` is 6 pt, `h-32` is 32 pt) and
// never an arbitrary value in brackets.

/** The spacing unit of the uniwind theme: `p-1` is this many points. */
export const SPACING_UNIT = 1;

/** Named distances of the design. */
export const SPACING = {
  /** Side padding of the transcript, the strip and the rows of the sheet. */
  gutter: 20,
  /** The gap between transcript items, turns and tool blocks (design item 8.3). */
  item: 12,
  /** Side insets of the nav row and the Settings list; the jump button's right edge. */
  inset: 16,
  /** Side insets of the body and the footer of the pairing screens. */
  pairing: 24,
  /** The smallest hit target. Header buttons, composer actions and Allow and Deny. */
  hit: 44,
  /** A hairline: separators, and the bottom of the bar and of the strip. */
  hairline: 0.5,
  /** The strip under the header (approval waiting, or the offline line). */
  strip: 34,
  /** The visual size of a composer action (Send, Stop) and of the capsules. */
  action: 32,
  /** The smallest height of the composer input (one line). */
  field: 40,
  /** The tallest composer input: six lines. Then the text scrolls inside it. */
  fieldMax: 145,
  /** The height of a primary or a bordered pairing button. */
  button: 50,
} as const;

/** Named corner radii of the design. Circles and capsules use `rounded-full`. */
export const RADII = {
  /** Tool, code and approval boxes; Allow and Deny. */
  box: 10,
  /** The diff box inside an approval block. */
  inner: 8,
  /** Pairing buttons, the command box and the key and value box. */
  control: 12,
  /** The search field. */
  field: 20,
  /** The groups of Settings. */
  group: 22,
  /** The glass pill of the composer: half of its one-line height of 48 pt. */
  composer: 24,
} as const;

/** The width of a hairline, in points. */
export const HAIRLINE = SPACING.hairline;

/** The width of the border of boxes, fields and glass buttons, in points. */
export const BORDER = 1;

/** The number column of a diff, the same in every diff (design item 8.9). */
export const DIFF_GUTTER = {
  width: 28,
  paddingRight: 6,
  /** The space between the number column and the code. */
  codeInset: 4,
} as const;

/**
 * The frame of a code block in Markdown (design 3.5). Our own view draws it around an
 * enriched text that holds only the fence (FENCE_MARKDOWN_STYLE of markdown.ts).
 */
export const CODE_BLOCK = {
  radius: RADII.box,
  headerHeight: 32,
  header: { paddingLeft: 12, paddingRight: 4 },
  body: { paddingTop: 8, paddingHorizontal: 12, paddingBottom: 10 },
  copyButton: { width: 36, height: 32 },
  /** The space under a code block, as under a paragraph. */
  marginBottom: 8,
} as const;

/** The block cursors, width and height. The shell one fits the 13/19 line (item 8.1). */
export const CURSOR = {
  shell: { width: 9, height: 19 },
  prose: { width: 7, height: 14 },
  code: { width: 6, height: 13 },
} as const;

/** The three sizes of the braille loader, width and height. */
export const BRAILLE = {
  /** Inline: tool status, the running line and the thought row. */
  inline: { width: 7, height: 13 },
  /** The composer Send button and the rows of the sheet. */
  row: { width: 8, height: 15 },
  /** The Allow button while it sends. */
  button: { width: 9, height: 17 },
} as const;

/** The dots that mark an approval: 7 pt in the header and strip, 8 pt in blocks and rows. */
export const DOT = {
  small: 7,
  large: 8,
} as const;

/**
 * Markdown lists: the text starts 20 pt in. Enriched puts the marker column (at least
 * `markerWidth`) and then the gap before the text, so 8 + 12 gives the 20 of the design.
 */
export const LIST = {
  markerWidth: 8,
  gap: 12,
  /** The step of a nested list. */
  nest: 20,
  /** The space between list items. */
  itemSpacing: 3,
} as const;
