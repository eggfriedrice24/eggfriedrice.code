// The edit block of a call that changed a file (design 3.8 and the S-Diff sheet): the
// tool block's box with the header "edit", the path, "+14" and "−3".
//
// - Collapsed: a preview of 4 lines and "show full diff · 3 hunks".
// - Expanded, in place (nothing is pushed): every hunk with its header and two number
//   columns, then "collapse". A diff over 400 lines shows its first 3 hunks and "show
//   remaining 9 hunks".
//
// The open states live in the row state of the conversation ("expanded" and
// "all_hunks"), because the list recycles row views.

import { Pressable, Text, View } from "react-native";

import { Hairline } from "@/components/ui/hairline";
import { PlainButton } from "@/components/ui/plain-button";
import type { ToolCallView } from "@/lib/stream/reducer";
import {
  diffLabel,
  diffModel,
  expandLabel,
  previewRows,
  remainingLabel,
  shownHunks,
  statTexts,
} from "../model/diff-view";
import { DiffHunks, DiffPreview } from "./diff-view";

export interface EditBlockViewProps {
  readonly call: ToolCallView;
  /** The unified diff of the call. */
  readonly diff: string;
  readonly expanded: boolean;
  /** True when a big diff shows all its hunks. */
  readonly allHunks: boolean;
  readonly onToggle: () => void;
  readonly onShowAllHunks: () => void;
}

/** An edit block. */
export function EditBlockView(props: EditBlockViewProps) {
  const model = diffModel(props.diff, props.call.summary);
  const stat = statTexts(model);
  const expanded = props.expanded;
  const shown = shownHunks(model, props.allHunks);
  return (
    <View className="overflow-hidden rounded-box border border-border-subtle">
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={diffLabel(model)}
        accessibilityState={{ expanded }}
        accessibilityHint={expanded ? "Shows the preview." : "Shows the full diff."}
        onPress={props.onToggle}
        className="min-h-34 flex-row items-center gap-8 px-12"
      >
        <Text className="text-mono-xs text-text-subtle">edit</Text>
        <Text
          numberOfLines={1}
          ellipsizeMode="middle"
          className="flex-1 text-mono-xs text-text-bright"
        >
          {model.path}
        </Text>
        {props.call.isError ? <Text className="text-mono-xs text-state-error">✗</Text> : null}
        <Text className="text-mono-xs text-diff-added">{stat.added}</Text>
        <Text className="text-mono-xs text-diff-removed">{stat.removed}</Text>
      </Pressable>
      {expanded ? (
        <>
          <Hairline />
          <DiffHunks hunks={shown.hunks} />
          {shown.hidden > 0 ? (
            <>
              <Hairline />
              <PlainButton
                label={remainingLabel(shown.hidden)}
                height="row"
                wide
                onPress={props.onShowAllHunks}
              />
            </>
          ) : null}
          <Hairline />
          <PlainButton label="collapse" height="row" wide onPress={props.onToggle} />
        </>
      ) : (
        <>
          <Hairline />
          <DiffPreview rows={previewRows(model)} />
          <Hairline />
          <PlainButton label={expandLabel(model)} height="row" wide onPress={props.onToggle} />
        </>
      )}
    </View>
  );
}
