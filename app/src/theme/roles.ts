// The colour roles of the app. Each role names one entry of palette.ts and has no value
// of its own, so the design system stays the only source of colour. The names follow the
// design canvas: a role of `semantic` keeps its name, and the Markdown, syntax and diff
// roles keep the names of `apps.opencode`. scripts/gen-theme.mjs makes one uniwind colour
// token of each role (`text_muted` gives `text-text-muted`, `bg-text-muted` and
// `border-text-muted`). A prop that cannot take a class reads this object.

import { opencode, semantic, treesitter } from "./palette";

/** The colour roles of the app, each one a value of palette.ts. */
export const roles = {
  // Surfaces.
  /** The backdrop of a sheet, and the base of the dim layer. */
  surface_0: semantic.surface_0,
  /** The background of every screen. */
  surface_1: semantic.surface_1,
  /** The groups of Settings, the fill of glass buttons and the track of a scroll bar. */
  surface_2: semantic.surface_2,
  /** The fill of a button while it is pressed or while its menu is open. */
  selection_bg: semantic.selection_bg,

  // Text.
  /** Body text, the composer text and the titles of rows. */
  text_primary: semantic.text_primary,
  /** Secondary text: values, the cwd, menu subtitles and the network loader. */
  text_muted: semantic.text_muted,
  /** Quiet text: placeholders, mono hints, tags and the status line. */
  text_subtle: semantic.text_subtle,
  /** Your messages, subjects of tool blocks, the approval command and button labels. */
  text_bright: semantic.text_bright,
  /** Text and glyphs on a yolk fill (Send, Allow, Steer, the primary button). */
  text_on_accent: semantic.text_on_accent,
  /** Line numbers. Decorative only: under 4.5:1, so never the only carrier of a meaning. */
  text_line_number: semantic.text_line_number,
  /** Disabled glyphs (Send disabled, copy disabled). Decorative only, as above. */
  text_decorative: semantic.text_decorative,

  // Accents.
  /** Yolk: the cursor, the ❯ prompt, Allow, Send, the efr loader and the approval dot. */
  accent_primary: semantic.accent_primary,
  /** Actions inside text: withdraw, retry, copy error, "show all" and "answer on". */
  accent_ui: semantic.accent_ui,
  /** The cwd in the shell block. It is the `prompt_directory` colour of the shell. */
  prompt_directory: semantic.accent_ui,

  // Borders.
  /** Every hairline and the 1 pt border of boxes, fields and glass buttons. */
  border_subtle: opencode.border_subtle,
  /** Borders of Deny, Stop and Cancel, the grabber, Markdown rules and scroll thumbs. */
  border_default: semantic.border_default,
  /** The outline of an approval block: the only gold outline in the app. */
  border_accent: semantic.border_accent,

  // States.
  /** ✓ glyphs, "connected" and "paired". */
  state_success: semantic.state_success,
  /** ✗ glyphs, a failed turn, the "error" words of stderr and destructive menu items. */
  state_error: semantic.state_error,
  /** The warning callouts of Markdown. */
  state_warning: semantic.state_warning,
  /** The note callouts of Markdown. */
  state_info: semantic.state_info,
  /** The tip callouts of Markdown. */
  state_hint: semantic.state_hint,
  /** The important callouts of Markdown. It is the `secondary` role of `apps.opencode`. */
  accent_secondary: opencode.secondary,

  // Markdown.
  markdown_text: opencode.markdown_text,
  markdown_heading: opencode.markdown_heading,
  markdown_strong: opencode.markdown_strong,
  markdown_emph: opencode.markdown_emph,
  markdown_code: opencode.markdown_code,
  /** The text of a link. */
  markdown_link_text: opencode.markdown_link_text,
  /** The underline of a link in the design. */
  markdown_link: opencode.markdown_link,
  markdown_block_quote: opencode.markdown_block_quote,
  markdown_list_item: opencode.markdown_list_item,
  markdown_list_enumeration: opencode.markdown_list_enumeration,
  markdown_horizontal_rule: opencode.markdown_horizontal_rule,
  /** The text of a code block body. The design draws it in the body colour, not green. */
  markdown_code_block: semantic.text_primary,

  // Code tokens.
  syntax_keyword: opencode.syntax_keyword,
  syntax_function: opencode.syntax_function,
  syntax_type: opencode.syntax_type,
  syntax_number: opencode.syntax_number,
  syntax_constant: semantic.code_constant,
  syntax_string: opencode.syntax_string,
  syntax_variable: opencode.syntax_variable,
  syntax_operator: opencode.syntax_operator,
  syntax_punctuation: opencode.syntax_punctuation,
  syntax_comment: opencode.syntax_comment,
  syntax_escape: semantic.code_escape,
  syntax_property: treesitter.property,
  syntax_attribute: treesitter.attribute,
  syntax_tag: treesitter.tag,

  // Diffs.
  diff_added: opencode.diff_added,
  diff_removed: opencode.diff_removed,
  diff_context: opencode.diff_context,
  diff_hunk_header: opencode.diff_hunk_header,
  /** The bold changed run of an added line. */
  diff_highlight_added: opencode.diff_highlight_added,
  /** The bold changed run of a removed line. */
  diff_highlight_removed: opencode.diff_highlight_removed,
  diff_added_bg: opencode.diff_added_bg,
  diff_removed_bg: opencode.diff_removed_bg,
  /** A fill for a changed run. The design does not use it now (design item x1). */
  diff_text_bg: semantic.diff_text_bg,
  diff_line_number: opencode.diff_line_number,
} as const;

/** The name of a colour role. */
export type Role = keyof typeof roles;

/**
 * No fill. It is not a colour: it removes a default fill, where the design draws a box
 * with no fill (Markdown code and tables, for example).
 */
export const NO_FILL = "transparent";

/**
 * A role colour with an opacity, as `rgba(...)`. The design draws materials, dim layers
 * and the cursor of the no-machine root as a role at an opacity, never as a new colour.
 */
export function withOpacity(role: Role, opacity: number): string {
  const hex = roles[role];
  const red = Number.parseInt(hex.slice(1, 3), 16);
  const green = Number.parseInt(hex.slice(3, 5), 16);
  const blue = Number.parseInt(hex.slice(5, 7), 16);
  return `rgba(${red}, ${green}, ${blue}, ${opacity})`;
}
