// The S-ModelSheet, MS-Chosen and MS-SignedOut boards of the gallery: the real model
// sheet in its states, drawn from the accounts of the boards (model/demo-models.ts)
// through the models of model-sheet.ts. The controls change nothing.

import { useState } from "react";
import { View } from "react-native";

import { ModelSheetView } from "@/features/conversation/components/model-sheet";
import {
  currentSettings,
  sheetView,
  type SheetInput,
  type SheetView,
} from "@/features/conversation/model/model-sheet";
import { demoModels } from "../model/demo-models";
import { SheetCase, SheetScroll } from "./sheet-layout";

interface ModelCase {
  readonly name: string;
  readonly view: SheetView;
  readonly fallback: boolean;
}

/** The states of the boards, at `now`. */
function modelCases(now: number): ModelCase[] {
  const list = demoModels(now);
  const input = (extra: Partial<SheetInput>): SheetInput => ({
    list,
    current: currentSettings(list, undefined, undefined),
    selection: undefined,
    machineName: "desktop",
    now,
    running: false,
    asks: true,
    cost: undefined,
    costPending: false,
    ...extra,
  });
  const b = { model: "gpt-6.1-sol", provider: "openai-subscription.b", effort: "medium" };
  const plan = { model: "gpt-6.1-sol", provider: "openai-subscription", effort: "high" };
  const cases: [string, SheetInput, boolean][] = [
    ["the default, nothing changed", input({}), false],
    ["a switch of account, cold cache", input({ selection: plan, cost: "cold_cache" }), false],
    ["the cost is asked for", input({ selection: plan, costPending: true }), false],
    ["a switch that compacts", input({ selection: plan, cost: "compacts" }), false],
    ["while a turn runs: next message", input({ selection: plan, running: true }), false],
    [
      "chosen here, with Use default",
      input({ current: currentSettings(list, b, undefined) }),
      false,
    ],
    [
      "a model that is gone",
      input({
        current: currentSettings(list, undefined, {
          model: "claude-sonnet-4-5",
          provider: "anthropic-api.work",
          effort: undefined,
        }),
      }),
      false,
    ],
    ["the effort steps as buttons (no native slider)", input({ selection: plan }), true],
  ];
  return cases.map(([name, one, fallback]) => ({ name, view: sheetView(one), fallback }));
}

/** The model sheet boards. */
export function ModelsSheet() {
  const [cases] = useState(() => modelCases(Date.now()));
  return (
    <SheetScroll>
      {cases.map((one) => (
        <SheetCase key={one.name} name={one.name}>
          <View className="overflow-hidden rounded-box border border-border-subtle">
            <ModelSheetView
              view={one.view}
              empty={undefined}
              machineName="desktop"
              quiet={false}
              onRow={() => undefined}
              onEffort={() => undefined}
              onUseDefault={() => undefined}
              onQuiet={() => undefined}
              onConfirm={() => undefined}
              fallbackSlider={one.fallback}
            />
          </View>
        </SheetCase>
      ))}
    </SheetScroll>
  );
}
