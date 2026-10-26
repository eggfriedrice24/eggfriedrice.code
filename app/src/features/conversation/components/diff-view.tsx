// The lines of a diff (design 3.8 and the S-Diff sheet), in mono 11/17 and the diff
// roles: added lines in diff_added on diff_added_bg, removed lines in diff_removed on
// diff_removed_bg, context in diff_context, hunk headers in diff_hunk_header, and the
// changed run of a pair bold in the highlight role. Line numbers are text_line_number,
// decoration only, in a 28 pt column (design item 8.9).
//
// - DiffPreview: the collapsed preview, one number column, lines cut at the edge.
// - DiffHunks: the whole diff, two number columns (old and new). Each hunk scrolls
//   sideways as one piece, and the numbers stay in place beside it.
//
// VoiceOver reads each line with "added" or "removed" (model/diff-view.ts lineLabel).

import { ScrollView, Text, View } from "react-native";

import { cn } from "@/components/ui/cn";
import type { DiffLineKind } from "../model/diff-lines";
import { type DiffHunk, type DiffRow, lineLabel, previewNumber } from "../model/diff-view";

const LINE_CLASS = {
  meta: "text-diff-context",
  file: "text-diff-context",
  hunk: "text-diff-hunk-header",
  added: "text-diff-added",
  removed: "text-diff-removed",
  context: "text-diff-context",
  note: "text-diff-context",
  other: "text-diff-context",
} as const satisfies Record<DiffLineKind, string>;

const ROW_FILL = {
  meta: "",
  file: "",
  hunk: "",
  added: "bg-diff-added-bg",
  removed: "bg-diff-removed-bg",
  context: "",
  note: "",
  other: "",
} as const satisfies Record<DiffLineKind, string>;

const CHANGED_CLASS = {
  added: "font-bold text-diff-highlight-added",
  removed: "font-bold text-diff-highlight-removed",
} as const;

/** The number column of a diff: 28 pt with 6 pt on its right (theme DIFF_GUTTER). */
const NUMBER_CLASS = "w-28 pr-6 text-right text-mono-diff text-text-line-number";

/** The text of one line, with its changed run marked. */
function LineText(props: { readonly row: DiffRow }) {
  const kind = props.row.line.kind;
  const changed = kind === "added" || kind === "removed" ? CHANGED_CLASS[kind] : "";
  return (
    <Text
      numberOfLines={1}
      ellipsizeMode="clip"
      className={cn("pl-4 text-mono-diff", LINE_CLASS[kind])}
    >
      {props.row.spans.map((span) =>
        span.changed ? (
          <Text key={span.start} className={changed}>
            {span.text}
          </Text>
        ) : (
          span.text
        ),
      )}
      {props.row.line.text === "" ? " " : null}
    </Text>
  );
}

function numberText(value: number | undefined): string {
  return value === undefined ? "" : String(value);
}

export interface DiffPreviewProps {
  readonly rows: readonly DiffRow[];
}

/** The collapsed preview of a diff: a few lines, one number column. */
export function DiffPreview(props: DiffPreviewProps) {
  return (
    <View className="py-4">
      {props.rows.map((row) => (
        <View
          key={row.id}
          accessible
          accessibilityLabel={lineLabel(row.line)}
          className={cn("flex-row", ROW_FILL[row.line.kind])}
        >
          <Text className={NUMBER_CLASS}>{numberText(previewNumber(row.line))}</Text>
          <View className="flex-1 overflow-hidden">
            <LineText row={row} />
          </View>
        </View>
      ))}
    </View>
  );
}

export interface DiffHunksProps {
  readonly hunks: readonly DiffHunk[];
}

/** The whole diff: each hunk with its header and two number columns. */
export function DiffHunks(props: DiffHunksProps) {
  return (
    <View className="py-4">
      {props.hunks.map((hunk) => (
        <Hunk key={hunk.id} hunk={hunk} />
      ))}
    </View>
  );
}

function Hunk(props: { readonly hunk: DiffHunk }) {
  const rows = props.hunk.rows;
  return (
    <View>
      {props.hunk.header === undefined ? null : (
        <Text numberOfLines={1} className="px-12 py-2 text-mono-diff text-diff-hunk-header">
          {props.hunk.header}
        </Text>
      )}
      <View className="flex-row">
        <View accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
          {rows.map((row) => (
            <View key={row.id} className={cn("flex-row", ROW_FILL[row.line.kind])}>
              <Text className={NUMBER_CLASS}>{numberText(row.line.oldLine)}</Text>
              <Text className={NUMBER_CLASS}>{numberText(row.line.newLine)}</Text>
            </View>
          ))}
        </View>
        <ScrollView
          horizontal
          showsHorizontalScrollIndicator
          indicatorStyle="white"
          contentContainerStyle={GROW}
          className="flex-1"
        >
          <View className="grow">
            {rows.map((row) => (
              <View
                key={row.id}
                accessible
                accessibilityLabel={lineLabel(row.line)}
                className={cn("pr-12", ROW_FILL[row.line.kind])}
              >
                <LineText row={row} />
              </View>
            ))}
          </View>
        </ScrollView>
      </View>
    </View>
  );
}

const GROW = { flexGrow: 1 } as const;
