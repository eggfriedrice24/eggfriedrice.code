// The Markdown style of the transcript: each colour is a role of the design system
// (palette.ts), never a value of its own. Markdown elements take the `markdown_*` roles
// of `apps.opencode`, code tokens its `syntax_*` roles, and the three tokens that it has
// no role for (property, attribute, tag) the tree-sitter captures of eggfriedrice.nvim.
// The `embedded` token has no capture there either, so it keeps the code colour.

import type { MarkdownStyle } from "react-native-enriched-markdown";

import { opencode, semantic, treesitter } from "./palette";

/** The monospace font of code on iOS. */
export const MONO_FONT = "Menlo";

/** The size of body text, in points. */
export const BODY_SIZE = 16;

/** The size of code, in points. */
export const CODE_SIZE = 13;

const heading = (fontSize: number) => ({
  color: opencode.markdown_heading,
  fontSize,
  fontWeight: "700",
  marginTop: 12,
  marginBottom: 6,
});

/** The 14 token colours of enriched's code highlighter. */
export const SYNTAX_COLORS = {
  keyword: opencode.syntax_keyword,
  operator: opencode.syntax_operator,
  punctuation: opencode.syntax_punctuation,
  string: opencode.syntax_string,
  number: opencode.syntax_number,
  constant: semantic.code_constant,
  comment: opencode.syntax_comment,
  function: opencode.syntax_function,
  type: opencode.syntax_type,
  variable: opencode.syntax_variable,
  property: treesitter.property,
  tag: treesitter.tag,
  attribute: treesitter.attribute,
} as const;

/** The style of every assistant message. */
export const MARKDOWN_STYLE: MarkdownStyle = {
  paragraph: {
    color: opencode.markdown_text,
    fontSize: BODY_SIZE,
    lineHeight: 23,
    marginBottom: 10,
  },
  h1: heading(22),
  h2: heading(20),
  h3: heading(18),
  h4: heading(17),
  h5: heading(16),
  h6: heading(16),
  strong: { color: opencode.markdown_strong, fontWeight: "bold" },
  em: { color: opencode.markdown_emph, fontStyle: "italic" },
  link: { color: opencode.markdown_link_text, underline: true },
  code: {
    color: opencode.markdown_code,
    backgroundColor: opencode.background_element,
    fontFamily: MONO_FONT,
    fontSize: CODE_SIZE,
  },
  codeBlock: {
    color: opencode.markdown_code_block,
    backgroundColor: opencode.background_panel,
    borderColor: opencode.border_subtle,
    borderWidth: 1,
    borderRadius: 8,
    padding: 10,
    fontFamily: MONO_FONT,
    fontSize: CODE_SIZE,
    lineHeight: 18,
    marginBottom: 10,
    syntaxColors: SYNTAX_COLORS,
  },
  blockquote: {
    color: opencode.markdown_block_quote,
    borderColor: opencode.markdown_block_quote,
    borderWidth: 3,
    gapWidth: 10,
    marginBottom: 10,
  },
  list: {
    color: opencode.markdown_text,
    fontSize: BODY_SIZE,
    bulletColor: opencode.markdown_list_item,
    markerColor: opencode.markdown_list_enumeration,
    marginBottom: 10,
  },
  thematicBreak: { color: opencode.markdown_horizontal_rule, height: 1 },
  table: {
    color: opencode.markdown_text,
    fontSize: 14,
    headerTextColor: opencode.markdown_strong,
    headerBackgroundColor: opencode.background_element,
    borderColor: opencode.border,
    borderWidth: 1,
    borderRadius: 6,
    marginBottom: 10,
  },
  taskList: {
    checkedColor: opencode.markdown_list_item,
    borderColor: opencode.markdown_list_item,
    checkmarkColor: semantic.text_on_accent,
  },
  strikethrough: { color: opencode.text_muted },
};
