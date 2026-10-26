// The diff box of a file write in an approval block (design 3.11 and S-Approvals): radius
// 8, a 1 pt border_subtle edge, padding 4 0, mono 11/17. The number column is the one
// diff gutter of the app (28 pt, padding-right 6, code inset 4; design item 8.9). The code
// scrolls sideways as one piece and the numbers stay in place, so a long line is never
// cut. VoiceOver reads "added" or "removed" before each changed line.

import { ScrollView, Text, View } from "react-native";

import { cn } from "@/components/ui/cn";
import { SEMIBOLD } from "@/theme/type";
import type { DiffRow } from "../model/approval-block";

/** The fill of each row kind. */
const ROW_FILL = {
  added: "bg-diff-added-bg",
  removed: "bg-diff-removed-bg",
  context: "",
  hunk: "",
  note: "",
  other: "",
} as const satisfies Record<DiffRow["kind"], string>;

/** The text colour of each row kind. */
const ROW_TEXT = {
  added: "text-diff-added",
  removed: "text-diff-removed",
  context: "text-diff-context",
  hunk: "text-diff-hunk-header",
  note: "text-diff-context",
  other: "text-diff-context",
} as const satisfies Record<DiffRow["kind"], string>;

/** The colour of the strong changed run of each row kind. */
const STRONG_TEXT = {
  added: "text-diff-highlight-added",
  removed: "text-diff-highlight-removed",
  context: "",
  hunk: "",
  note: "",
  other: "",
} as const satisfies Record<DiffRow["kind"], string>;

/** The weight of a changed run. A weight cannot be a class of the theme, so it is a style. */
const STRONG = { fontWeight: SEMIBOLD } as const;

const SPOKEN = {
  added: "added",
  removed: "removed",
  context: undefined,
  hunk: undefined,
  note: undefined,
  other: undefined,
} as const satisfies Record<DiffRow["kind"], string | undefined>;

export interface ApprovalDiffProps {
  readonly rows: readonly DiffRow[];
}

/** The diff box. */
export function ApprovalDiff(props: ApprovalDiffProps) {
  return (
    <View
      testID="approval-diff"
      className="mt-10 flex-row overflow-hidden rounded-inner border border-border-subtle py-4"
    >
      <View>
        {props.rows.map((row) => (
          <Text
            key={row.key}
            accessibilityElementsHidden
            importantForAccessibility="no-hide-descendants"
            className={cn(
              "text-mono-diff w-28 pr-6 text-right text-diff-line-number",
              ROW_FILL[row.kind],
            )}
          >
            {row.number}
          </Text>
        ))}
      </View>
      <ScrollView
        horizontal
        showsHorizontalScrollIndicator={false}
        className="flex-1"
        contentContainerClassName="grow"
      >
        <View className="grow">
          {props.rows.map((row) => {
            const spoken = SPOKEN[row.kind];
            const text = row.sign + row.runs.map((run) => run.text).join("");
            return (
              <Text
                key={row.key}
                accessibilityLabel={spoken === undefined ? undefined : `${spoken}: ${text}`}
                className={cn("text-mono-diff pl-4 pr-8", ROW_FILL[row.kind], ROW_TEXT[row.kind])}
              >
                {row.sign}
                {row.runs.map((run) =>
                  run.strong ? (
                    <Text key={run.at} className={STRONG_TEXT[row.kind]} style={STRONG}>
                      {run.text}
                    </Text>
                  ) : (
                    run.text
                  ),
                )}
              </Text>
            );
          })}
        </View>
      </ScrollView>
    </View>
  );
}
