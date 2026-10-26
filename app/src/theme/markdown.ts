// The Markdown style of the transcript (design 3.5 and the S-Markdown sheet). Each colour
// is a role of roles.ts, so a value of the design system, never a value of its own.
// Markdown elements take the `markdown_*` roles, code tokens the `syntax_*` roles. Each
// colour that enriched has a default for is set here, so no default of enriched shows.
//
// Limits of enriched 1.1.1, so the style differs from the design here:
// - A link underline takes the colour of the link text: enriched sets the underline
//   colour to the link colour, so the `markdown_link` underline of the design cannot
//   show.
// - Strong text is bold, not semibold: enriched has only "bold" and "normal" for it.
// - A table has one border width for every line, so the line under the header is a
//   hairline too, and the horizontal cell padding is the same on both sides.
// - A code comment is not italic, and `escape` has no token of its own.

import type { MarkdownStyle } from "react-native-enriched-markdown";

import { BORDER, CODE_BLOCK, HAIRLINE, LIST } from "./metrics";
import { NO_FILL, roles } from "./roles";
import { SEMIBOLD, TYPE } from "./type";

/**
 * The font of code blocks inside enriched. Enriched makes its fonts with the old RCTFont
 * of React Native, which does not know `ui-monospace` and falls back to SF Pro, so code
 * blocks use Menlo. Inline code names no font: enriched then takes the monospaced
 * system font (SF Mono).
 */
export const CODE_BLOCK_FONT = "Menlo";

/** The 13 token colours of enriched's code highlighter. `embedded` keeps the code colour. */
export const SYNTAX_COLORS = {
  keyword: roles.syntax_keyword,
  operator: roles.syntax_operator,
  punctuation: roles.syntax_punctuation,
  string: roles.syntax_string,
  number: roles.syntax_number,
  constant: roles.syntax_constant,
  comment: roles.syntax_comment,
  function: roles.syntax_function,
  type: roles.syntax_type,
  variable: roles.syntax_variable,
  property: roles.syntax_property,
  tag: roles.syntax_tag,
  attribute: roles.syntax_attribute,
} as const;

const heading = (style: { fontSize: number; lineHeight: number }) => ({
  color: roles.markdown_heading,
  fontSize: style.fontSize,
  lineHeight: style.lineHeight,
  fontWeight: SEMIBOLD,
});

const BLOCK_GAP = 8;

/** The style of every assistant message. */
export const MARKDOWN_STYLE: MarkdownStyle = {
  paragraph: {
    color: roles.markdown_text,
    fontSize: TYPE.body.fontSize,
    lineHeight: TYPE.body.lineHeight,
    marginTop: 0,
    marginBottom: BLOCK_GAP,
  },
  h1: { ...heading(TYPE.heading1), marginTop: 0, marginBottom: 8 },
  h2: { ...heading(TYPE.heading2), marginTop: 16, marginBottom: 6 },
  h3: { ...heading(TYPE.heading3), marginTop: 14, marginBottom: 4 },
  h4: { ...heading(TYPE.heading3), marginTop: 14, marginBottom: 4 },
  h5: { ...heading(TYPE.heading3), marginTop: 14, marginBottom: 4 },
  h6: { ...heading(TYPE.heading3), marginTop: 14, marginBottom: 4 },
  strong: { color: roles.markdown_strong, fontWeight: "bold" },
  em: { color: roles.markdown_emph, fontStyle: "italic" },
  link: { color: roles.markdown_link_text, underline: true, backgroundColor: NO_FILL },
  underline: { color: roles.markdown_text },
  strikethrough: { color: roles.text_muted },
  code: {
    color: roles.markdown_code,
    backgroundColor: NO_FILL,
    borderColor: NO_FILL,
    fontSize: TYPE.monoInline.fontSize,
  },
  // A fence that our code block frame does not hold (one inside a list, for example)
  // gets the same box from enriched, without the header.
  codeBlock: {
    color: roles.markdown_code_block,
    backgroundColor: NO_FILL,
    borderColor: roles.border_subtle,
    borderWidth: BORDER,
    borderRadius: CODE_BLOCK.radius,
    padding: CODE_BLOCK.body.paddingHorizontal,
    fontFamily: CODE_BLOCK_FONT,
    fontSize: TYPE.monoCode.fontSize,
    lineHeight: TYPE.monoCode.lineHeight,
    marginTop: 0,
    marginBottom: CODE_BLOCK.marginBottom,
    syntaxColors: SYNTAX_COLORS,
  },
  blockquote: {
    color: roles.markdown_block_quote,
    fontSize: TYPE.body.fontSize,
    lineHeight: TYPE.body.lineHeight,
    borderColor: roles.border_default,
    borderWidth: 2,
    gapWidth: 12,
    backgroundColor: NO_FILL,
    borderRadius: 0,
    padding: 0,
    marginTop: 0,
    marginBottom: BLOCK_GAP,
    admonitions: {
      note: { color: roles.state_info, backgroundColor: NO_FILL },
      tip: { color: roles.state_hint, backgroundColor: NO_FILL },
      important: { color: roles.accent_secondary, backgroundColor: NO_FILL },
      warning: { color: roles.state_warning, backgroundColor: NO_FILL },
      caution: { color: roles.state_error, backgroundColor: NO_FILL },
    },
  },
  list: {
    color: roles.markdown_text,
    fontSize: TYPE.body.fontSize,
    lineHeight: TYPE.body.lineHeight,
    bulletColor: roles.markdown_list_item,
    markerColor: roles.markdown_list_enumeration,
    markerMinWidth: LIST.markerWidth,
    gapWidth: LIST.gap,
    marginLeft: LIST.nest,
    itemSpacing: LIST.itemSpacing,
    marginTop: 0,
    marginBottom: BLOCK_GAP,
  },
  thematicBreak: {
    color: roles.markdown_horizontal_rule,
    height: BORDER,
    marginTop: 14,
    marginBottom: 14,
  },
  table: {
    color: roles.markdown_text,
    fontSize: TYPE.table.fontSize,
    lineHeight: TYPE.table.lineHeight,
    headerTextColor: roles.markdown_strong,
    headerBackgroundColor: NO_FILL,
    rowEvenBackgroundColor: NO_FILL,
    rowOddBackgroundColor: NO_FILL,
    borderColor: roles.border_subtle,
    borderWidth: HAIRLINE,
    borderRadius: 0,
    cellPaddingVertical: 6,
    // Both sides of a cell, so 7 + 7 gives the 14 pt between columns of the design.
    cellPaddingHorizontal: 7,
    marginTop: 0,
    marginBottom: 10,
  },
  taskList: {
    checkedColor: roles.markdown_list_item,
    borderColor: roles.markdown_list_item,
    checkmarkColor: roles.text_on_accent,
    checkedTextColor: roles.text_muted,
  },
  highlight: { backgroundColor: roles.selection_bg },
  spoiler: { color: roles.text_subtle },
  video: { backgroundColor: roles.surface_0 },
  math: { color: roles.markdown_text, backgroundColor: NO_FILL },
  inlineMath: { color: roles.markdown_text },
};

/**
 * The style of the enriched text inside our code block frame: the frame draws the
 * border, the header and the padding, so the fence itself has none.
 */
export const FENCE_MARKDOWN_STYLE: MarkdownStyle = {
  ...MARKDOWN_STYLE,
  codeBlock: {
    ...MARKDOWN_STYLE.codeBlock,
    borderColor: NO_FILL,
    borderWidth: 0,
    borderRadius: 0,
    padding: 0,
    marginBottom: 0,
  },
};
