// The VoiceOver label of the title of the bar. The title opens the conversations sheet,
// and the label adds the waits elsewhere, which the yolk dot shows, in the words of the
// strip (C-Waiting 0.0.12). Pure.

/**
 * The label of the title `title` with `waiting`, the words of the waits elsewhere such
 * as "3 conversations need you", or undefined when none waits.
 */
export function titleLabel(title: string, waiting: string | undefined): string {
  const base = `${title}, all conversations`;
  return waiting === undefined ? base : `${base}, ${waiting}`;
}
