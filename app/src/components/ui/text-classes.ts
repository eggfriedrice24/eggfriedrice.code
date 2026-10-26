// The uniwind classes of the type scale and of the text colours. Tailwind finds a class
// only when the whole class name is in the source, so these maps spell every class out;
// a class built from parts (`text-${name}`) would make no style. text-classes.test.ts
// checks that each class is the one scripts/gen-theme.mjs makes from its key.

import type { Role } from "@/theme/roles";
import type { TypeName } from "@/theme/type";

/** The class of each style of the type scale (src/theme/type.ts). */
export const TYPE_CLASS = {
  body: "text-body",
  input: "text-input",
  row: "text-row",
  title: "text-title",
  reason: "text-reason",
  thought: "text-thought",
  caption: "text-caption",
  table: "text-table",
  heading1: "text-heading-1",
  heading2: "text-heading-2",
  heading3: "text-heading-3",
  pairingTitle: "text-pairing-title",
  button: "text-button",
  buttonLarge: "text-button-large",
  capsule: "text-capsule",
  monoXs: "text-mono-xs",
  monoXsTight: "text-mono-xs-tight",
  monoDiff: "text-mono-diff",
  monoSm: "text-mono-sm",
  monoCode: "text-mono-code",
  monoInline: "text-mono-inline",
  monoCommand: "text-mono-command",
  monoShell: "text-mono-shell",
  monoName: "text-mono-name",
  monoLarge: "text-mono-large",
  monoValue: "text-mono-value",
  monoTitle: "text-mono-title",
  monoHeading: "text-mono-heading",
  monoDisplay: "text-mono-display",
} as const satisfies Record<TypeName, string>;

/** A style of the type scale in SF Mono. */
export type MonoSize = Extract<TypeName, `mono${string}`>;

/** A style of the type scale in the system font (SF Pro). */
export type BodySize = Exclude<TypeName, MonoSize>;

/** The class of each colour role that text can have. */
export const TEXT_TONE_CLASS = {
  text_primary: "text-text-primary",
  text_muted: "text-text-muted",
  text_subtle: "text-text-subtle",
  text_bright: "text-text-bright",
  text_on_accent: "text-text-on-accent",
  text_line_number: "text-text-line-number",
  text_decorative: "text-text-decorative",
  accent_primary: "text-accent-primary",
  accent_ui: "text-accent-ui",
  prompt_directory: "text-prompt-directory",
  state_success: "text-state-success",
  state_error: "text-state-error",
  markdown_heading: "text-markdown-heading",
  diff_added: "text-diff-added",
  diff_removed: "text-diff-removed",
  diff_context: "text-diff-context",
  diff_hunk_header: "text-diff-hunk-header",
  diff_line_number: "text-diff-line-number",
} as const satisfies Partial<Record<Role, string>>;

/** A colour role that text can have. */
export type TextTone = keyof typeof TEXT_TONE_CLASS;
