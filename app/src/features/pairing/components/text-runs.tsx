// The runs of a pairing title or reason (model/pairing-view.ts) inside one text: plain
// words, a machine name or a version in SF Mono, and a command in SF Mono and yolk, as
// in the shell (P-Errors: "Codes from efr device add only work...").

import { Text } from "react-native";

import { cn } from "@/components/ui/cn";
import { MONO_FONT } from "@/theme/type";

import type { TextRun } from "../model/pairing-view";

export interface TextRunsProps {
  readonly runs: readonly TextRun[];
  /**
   * The class of a mono run. A title gives its mono style (19 pt in a 20 pt title); a
   * reason gives none, so the run keeps the size of the line.
   */
  readonly monoClassName?: string;
}

/** A run with the key of its place in the text. */
interface KeyedRun {
  readonly key: string;
  readonly run: TextRun;
}

/** The runs of `runs` with a key each: the offset of the run in the text. */
function keyed(runs: readonly TextRun[]): KeyedRun[] {
  const out: KeyedRun[] = [];
  let at = 0;
  for (const run of runs) {
    out.push({ key: String(at), run });
    at += run.text.length;
  }
  return out;
}

/** The runs, as the children of a Text. */
export function TextRuns(props: TextRunsProps) {
  const monoClass = props.monoClassName;
  return keyed(props.runs).map(({ key, run }) => {
    if (run.look === "plain") {
      return <Text key={key}>{run.text}</Text>;
    }
    // A mono run without a style of its own changes only the family, so it keeps the
    // size and the line height of the line around it.
    return (
      <Text
        key={key}
        className={cn(monoClass, run.look === "command" && "text-accent-primary")}
        style={monoClass === undefined ? { fontFamily: MONO_FONT } : undefined}
      >
        {run.text}
      </Text>
    );
  });
}
