// The Q-Quarantine board of the gallery: the strip and the sheet of the quarantine
// question of the reviewer off in each state, from the models of quarantine.ts. The
// cards of the question are in the AU-Auto transcript board. The controls change nothing.

import { useState } from "react";
import { View } from "react-native";

import type { SurfaceChange } from "@/lib/protocol/generated";
import type { QuarantineQuestionView } from "@/lib/stream/reducer";
import {
  QuarantineSheetView,
  QuarantineStripView,
} from "@/features/conversation/components/quarantine-sheet";
import {
  quarantineSheet,
  quarantineStrip,
  type QuarantineSheetModel,
  type QuarantineStripModel,
} from "@/features/conversation/model/quarantine";
import { DEMO_CWD } from "../model/demo-script";
import { SheetCase, SheetScroll } from "./sheet-layout";

const CHANGES: readonly SurfaceChange[] = [
  {
    path: `${DEMO_CWD}/.git/config`,
    rule: "config_code_key",
    key: "core.fsmonitor",
    quarantined: true,
  },
  { path: `${DEMO_CWD}/.git/hooks/pre-commit`, rule: "hook_planted", quarantined: true },
  { path: `${DEMO_CWD}/.git/commondir`, rule: "commondir_in_main_git_dir", quarantined: true },
  { path: `${DEMO_CWD}/.git/config.worktree`, rule: "config_worktree_appeared", quarantined: true },
];

function question(
  changes: readonly SurfaceChange[],
  answer: QuarantineQuestionView["answer"],
): QuarantineQuestionView {
  return {
    questionId: "0199d3e0-de38-7000-8000-000000000001",
    turnId: "0199d3e0-de38-7000-8000-000000000002",
    callId: "0199d3e0-de38-7000-8000-000000000003",
    changes,
    answer,
    origin: answer === "pending" || answer === "not_answered" ? undefined : "shell",
  };
}

interface QuarantineCase {
  readonly name: string;
  readonly strip: QuarantineStripModel | undefined;
  readonly sheet: QuarantineSheetModel;
}

function quarantineCases(): QuarantineCase[] {
  const one = CHANGES.slice(0, 1);
  const cases: [string, QuarantineQuestionView, boolean][] = [
    ["1, 2 · one change", question(one, "pending"), false],
    ["4 · four changes", question(CHANGES, "pending"), false],
    ["5 · answered on the terminal", question(CHANGES, "left"), false],
    ["kept on the terminal", question(one, "kept"), false],
    ["6 · not answered", question(one, "not_answered"), true],
  ];
  return cases.map(([name, asked, ended]) => ({
    name,
    strip: quarantineStrip(asked, DEMO_CWD, "desktop"),
    sheet: quarantineSheet(asked, DEMO_CWD, "desktop", ended),
  }));
}

/** The Q-Quarantine board. */
export function QuarantineBoard() {
  const [cases] = useState(quarantineCases);
  return (
    <SheetScroll>
      {cases.map((one) => (
        <SheetCase key={one.name} name={one.name}>
          <View className="gap-12">
            {one.strip === undefined ? null : (
              <View className="overflow-hidden rounded-box border border-border-subtle">
                <QuarantineStripView model={one.strip} onOpen={() => undefined} />
              </View>
            )}
            <View className="overflow-hidden rounded-box border border-border-subtle">
              <QuarantineSheetView model={one.sheet} onClose={() => undefined} />
            </View>
          </View>
        </SheetCase>
      ))}
    </SheetScroll>
  );
}
