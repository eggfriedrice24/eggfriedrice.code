// The VoiceOver label of the title of the bar. The title opens the conversations sheet,
// and the label adds the approvals that wait elsewhere, which the yolk dot shows. Pure.

/** The label of the title `title` while `waiting` approvals wait elsewhere. */
export function titleLabel(title: string, waiting: number): string {
  const base = `${title}, all conversations`;
  if (waiting <= 0) {
    return base;
  }
  return waiting === 1 ? `${base}, 1 approval waiting` : `${base}, ${waiting} approvals waiting`;
}
