// The colours of the eggfriedrice design system, copied without change from
// eggfriedrice.nvim extras/palette/eggfriedrice.json at commit 351bd0b
// (https://github.com/eggfriedrice24/eggfriedrice.nvim). The palette is dark only.
//
// Do not add a colour here that the source file does not have, and do not map a role to
// another role here. When the source file changes, copy the new values and update the
// commit above. The app reads colours only through these roles: `semantic`, `apps.efr`
// and `apps.opencode` of the source file, plus the three tree-sitter captures at the end
// of this file. The semantic roles of the terminal, the prompt, fzf, search, git and the
// shell are left out, because the app has no such parts.

/** The `semantic` roles of the design system. */
export const semantic = {
  text_primary: "#d8d3c3",
  text_muted: "#a8a396",
  text_subtle: "#8695b7",
  text_bright: "#f2eddd",
  text_on_accent: "#0d111a",
  text_line_number: "#586480",
  text_decorative: "#46516c",
  surface_0: "#090c13",
  surface_1: "#0d111a",
  surface_2: "#121722",
  accent_primary: "#ffc940",
  accent_ui: "#78e2d6",
  state_success: "#60e654",
  state_warning: "#ffc940",
  state_error: "#e06c75",
  state_info: "#78e2d6",
  state_hint: "#60e654",
  focus: "#ffc940",
  selection_bg: "#2f3b54",
  selection_fg: "#d8d3c3",
  border_default: "#586480",
  border_accent: "#c9a747",
  link: "#c084fc",
  diff_add_bg: "#192c17",
  diff_change_bg: "#302509",
  diff_delete_bg: "#391d1e",
  diff_text_bg: "#564825",
  code_keyword: "#c084fc",
  code_function: "#ffc940",
  code_type: "#ffc940",
  code_constant: "#ffc940",
  code_string: "#60e654",
  code_variable: "#e06c75",
  code_operator: "#6cb8ff",
  code_escape: "#78e2d6",
  code_decorator: "#c084fc",
  code_punctuation: "#a8a396",
  code_comment: "#8695b7",
} as const;

/** The `apps.efr` roles: the colours of efr in the terminal. */
export const efr = {
  text: "#d8d3c3",
  muted: "#a8a396",
  accent: "#ffc940",
  heading: "#e06c75",
  link: "#c084fc",
  code: "#60e654",
  success: "#60e654",
  warning: "#ffc940",
  error: "#e06c75",
  quote: "#8695b7",
  diff: {
    add: "#60e654",
    remove: "#e06c75",
    hunk: "#78e2d6",
  },
} as const;

/** The `apps.opencode` roles: Markdown, syntax and diff colours of a chat client. */
export const opencode = {
  primary: "#ffc940",
  secondary: "#c084fc",
  accent: "#78e2d6",
  error: "#e06c75",
  warning: "#ffc940",
  success: "#60e654",
  info: "#78e2d6",
  text: "#d8d3c3",
  text_muted: "#a8a396",
  background: "#0d111a",
  background_panel: "#090c13",
  background_element: "#121722",
  border: "#586480",
  border_active: "#c9a747",
  border_subtle: "#46516c",
  diff_added: "#60e654",
  diff_removed: "#e06c75",
  diff_context: "#8695b7",
  diff_hunk_header: "#78e2d6",
  diff_highlight_added: "#57f749",
  diff_highlight_removed: "#fb6c7a",
  diff_added_bg: "#192c17",
  diff_removed_bg: "#391d1e",
  diff_context_bg: "#0d111a",
  diff_line_number: "#586480",
  diff_added_line_number_bg: "#192c17",
  diff_removed_line_number_bg: "#391d1e",
  markdown_text: "#d8d3c3",
  markdown_heading: "#e06c75",
  markdown_link: "#c084fc",
  markdown_link_text: "#78e2d6",
  markdown_code: "#60e654",
  markdown_block_quote: "#8695b7",
  markdown_emph: "#d8d3c3",
  markdown_strong: "#f2eddd",
  markdown_horizontal_rule: "#586480",
  markdown_list_item: "#ffc940",
  markdown_list_enumeration: "#ffc940",
  markdown_image: "#c084fc",
  markdown_image_text: "#78e2d6",
  markdown_code_block: "#60e654",
  syntax_comment: "#8695b7",
  syntax_keyword: "#c084fc",
  syntax_function: "#ffc940",
  syntax_variable: "#e06c75",
  syntax_string: "#60e654",
  syntax_number: "#ffc940",
  syntax_type: "#ffc940",
  syntax_operator: "#6cb8ff",
  syntax_punctuation: "#a8a396",
} as const;

/**
 * The tree-sitter captures of eggfriedrice.nvim for the three code tokens that
 * `apps.opencode` has no syntax role for, copied from lua/eggfriedrice/treesitter.lua at
 * the same commit 351bd0b (`@property` line 29, `@attribute` line 67, `@tag` line 147),
 * with the values of their primitives in the source file (`red`, `purple`, `red`).
 */
export const treesitter = {
  property: "#e06c75",
  attribute: "#c084fc",
  tag: "#e06c75",
} as const;
