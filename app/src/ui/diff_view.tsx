// A unified diff, line by line, as efr's terminal shows it (diff_lines.ts): added lines
// in `diff_added` on `diff_added_bg`, removed lines in `diff_removed` on
// `diff_removed_bg`, hunk headers in `diff_hunk_header`, file headers bold, with the old
// and new line numbers in a gutter. Lines wrap, so no line needs a sideways scroll.

import { memo, useMemo } from "react";
import { StyleSheet, Text, View } from "react-native";

import { MONO_FONT } from "@/theme/markdown";
import { opencode, semantic } from "@/theme/palette";

import { type DiffLine, type DiffLineKind, diffLines, diffStat } from "./diff_lines";

/** The most lines that the view shows; the rest is one line that counts them. */
const MAX_LINES = 400;

interface NumberedLine {
  /** The place of the line in the diff: a diff never reorders its lines. */
  readonly id: number;
  readonly line: DiffLine;
}

/** A diff with its counts of added and removed lines. */
export const DiffView = memo(function DiffView(props: { readonly diff: string }) {
  const { lines, shown, stat, numbered } = useMemo(() => {
    const all = diffLines(props.diff);
    return {
      lines: all,
      shown: all.slice(0, MAX_LINES).map((line, id): NumberedLine => ({ id, line })),
      stat: diffStat(all),
      numbered: all.some((line) => line.oldLine !== undefined || line.newLine !== undefined),
    };
  }, [props.diff]);
  return (
    <View
      style={styles.frame}
      accessibilityLabel={`Diff: ${stat.added} lines added, ${stat.removed} lines removed`}
    >
      <Text style={styles.stat}>
        <Text style={styles.added}>+{stat.added}</Text>{" "}
        <Text style={styles.removed}>-{stat.removed}</Text>
      </Text>
      {shown.map((entry) => (
        <DiffRow key={entry.id} line={entry.line} numbered={numbered} />
      ))}
      {lines.length > shown.length ? (
        <Text style={styles.more}>{lines.length - shown.length} more lines</Text>
      ) : null}
    </View>
  );
});

function DiffRow(props: { readonly line: DiffLine; readonly numbered: boolean }) {
  const { line } = props;
  return (
    <View style={ROWS[line.kind]}>
      {props.numbered ? (
        <Text style={styles.gutter}>
          {pad(line.oldLine)} {pad(line.newLine)}
        </Text>
      ) : null}
      <Text style={TEXTS[line.kind]}>{line.text === "" ? " " : line.text}</Text>
    </View>
  );
}

function pad(value: number | undefined): string {
  const text = value === undefined ? "" : String(value);
  return text.padStart(4, " ");
}

const styles = StyleSheet.create({
  frame: {
    backgroundColor: opencode.diff_context_bg,
    borderColor: opencode.border_subtle,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: 8,
    paddingVertical: 6,
    overflow: "hidden",
  },
  stat: { fontFamily: MONO_FONT, fontSize: 12, paddingHorizontal: 8, paddingBottom: 4 },
  added: { color: opencode.diff_added },
  removed: { color: opencode.diff_removed },
  gutter: {
    fontFamily: MONO_FONT,
    fontSize: 12,
    lineHeight: 17,
    color: opencode.diff_line_number,
    marginRight: 8,
  },
  more: {
    fontFamily: MONO_FONT,
    fontSize: 12,
    color: semantic.text_muted,
    paddingHorizontal: 8,
    paddingTop: 4,
  },
});

const ROW = { flexDirection: "row", paddingHorizontal: 8 } as const;
const CODE = { flex: 1, fontFamily: MONO_FONT, fontSize: 12, lineHeight: 17 } as const;

/** The row style of each kind of line. */
const ROWS = StyleSheet.create<Record<DiffLineKind, typeof ROW & { backgroundColor?: string }>>({
  meta: ROW,
  file: ROW,
  hunk: ROW,
  added: { ...ROW, backgroundColor: opencode.diff_added_bg },
  removed: { ...ROW, backgroundColor: opencode.diff_removed_bg },
  context: ROW,
  note: ROW,
  other: ROW,
});

/** The text style of each kind of line. */
const TEXTS = StyleSheet.create({
  meta: { ...CODE, color: opencode.text, fontWeight: "700" },
  file: { ...CODE, color: opencode.text, fontWeight: "700" },
  hunk: { ...CODE, color: opencode.diff_hunk_header },
  added: { ...CODE, color: opencode.diff_added },
  removed: { ...CODE, color: opencode.diff_removed },
  context: { ...CODE, color: opencode.diff_context },
  note: { ...CODE, color: opencode.text_muted },
  other: { ...CODE, color: opencode.text },
});
