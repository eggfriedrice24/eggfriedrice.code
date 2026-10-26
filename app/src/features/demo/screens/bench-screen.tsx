// The decode bench of the dev builds (plan section 2.3, item 12; device check 4): it
// decodes every item of the 8 recordings of efrd 100 times in each mode (zod, jitless and
// the hand guard), and shows the first parse and the p50 and p95 per kind, with the
// draft path against its budget of 0.5 ms. The report can go to the clipboard.
//
// The run pauses between chunks, so the screen stays live; its numbers count only the
// parses. Open it right after the app starts, before a conversation decodes anything, to
// see the cold first parse.

import { Redirect, Stack } from "expo-router";
import { useState } from "react";
import { View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { CopyButton } from "@/components/ui/copy-button";
import { InsetGroup, InsetRow } from "@/components/ui/inset-group";
import { MonoText } from "@/components/ui/mono-text";
import { PrimaryButton } from "@/components/ui/primary-button";
import {
  BENCH_ROUNDS,
  DRAFT_BUDGET_MS,
  benchItems,
  micros,
  reportText,
  runDecodeBench,
  type BenchFacts,
  type BenchReport,
  type ModeReport,
} from "@/lib/dev/decode-bench";
import { BENCH_DECODERS, zodCompiles } from "@/lib/dev/decode-modes";
import { DEV_TOOLS } from "@/lib/dev/dev-tools";
import { engineName } from "@/lib/dev/native/engine";
import { highResolutionNow } from "@/lib/dev/native/high-resolution-now";
import { RECORDINGS } from "@/lib/dev/recordings";
import { SheetScroll } from "../components/sheet-layout";

/** Where the bench is. */
type BenchPhase =
  | { readonly kind: "idle" }
  | { readonly kind: "running"; readonly done: number; readonly total: number }
  | { readonly kind: "done"; readonly report: BenchReport }
  | { readonly kind: "failed"; readonly message: string };

/** A pause that lets the screen draw a frame. */
function pause(): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
}

export interface BenchScreenProps {
  /** The rounds over all items; the plan's 100 by default. A test runs fewer. */
  readonly rounds?: number;
}

/** The bench screen. A build without the dev tools goes to the root. */
export function BenchScreen(props: BenchScreenProps) {
  if (!DEV_TOOLS) {
    return <Redirect href="/" />;
  }
  return <Bench rounds={props.rounds ?? BENCH_ROUNDS} />;
}

function Bench(props: { readonly rounds: number }) {
  const [phase, setPhase] = useState<BenchPhase>({ kind: "idle" });
  const [clock] = useState(highResolutionNow);
  const [facts] = useState<BenchFacts>(() => ({
    engine: engineName(),
    compiles: zodCompiles(),
    precise: clock.precise,
  }));

  const start = (): void => {
    setPhase({ kind: "running", done: 0, total: 1 });
    runDecodeBench(benchItems(RECORDINGS), {
      decoders: BENCH_DECODERS,
      now: clock.now,
      rounds: props.rounds,
      pause,
      onProgress: (done, total) => setPhase({ kind: "running", done, total }),
    }).then(
      (report) => setPhase({ kind: "done", report }),
      (error: unknown) =>
        setPhase({
          kind: "failed",
          message: error instanceof Error ? error.message : String(error),
        }),
    );
  };

  return (
    <>
      <Stack.Screen options={{ title: "Decode bench" }} />
      <SheetScroll>
        <InsetGroup
          header="This phone"
          footer={`The budget of a draft frame is a p95 under ${micros(DRAFT_BUDGET_MS)}.`}
        >
          <InsetRow title="Engine" value={facts.engine} valueMono />
          <InsetRow
            title="zod compiles its parsers"
            value={facts.compiles ? "yes" : "no"}
            valueMono
          />
          <InsetRow title="Clock under 1 ms" value={facts.precise ? "yes" : "no"} valueMono />
        </InsetGroup>
        {phase.kind === "running" ? (
          <View className="flex-row items-center gap-6" accessibilityLabel="The bench runs">
            <BrailleCell tint="work" />
            <MonoText tone="text_muted">
              {`running · ${Math.floor((phase.done / phase.total) * 100)}%`}
            </MonoText>
          </View>
        ) : (
          <PrimaryButton
            label={phase.kind === "done" ? "Run again" : "Run the bench"}
            onPress={start}
          />
        )}
        {phase.kind === "failed" ? (
          <View role="alert">
            <MonoText tone="state_error">{phase.message}</MonoText>
          </View>
        ) : null}
        {phase.kind === "done" ? <Report report={phase.report} facts={facts} /> : null}
      </SheetScroll>
    </>
  );
}

interface ReportProps {
  readonly report: BenchReport;
  readonly facts: BenchFacts;
}

function Report(props: ReportProps) {
  const { report } = props;
  return (
    <>
      <View className="flex-row items-center justify-between">
        <MonoText tone="text_muted">{`${report.items} items x ${report.rounds} rounds`}</MonoText>
        <CopyButton
          variant="circle"
          text={() => reportText(report, props.facts)}
          label="Copy the report"
        />
      </View>
      {report.modes.map((mode) => (
        <ModeTable key={mode.mode} mode={mode} />
      ))}
    </>
  );
}

function ModeTable(props: { readonly mode: ModeReport }) {
  const { mode } = props;
  const verdict = mode.withinBudget ? "within the budget" : "over the budget";
  return (
    <InsetGroup
      header={`${mode.mode}: draft p95 ${micros(mode.draftP95Ms)}, ${verdict}`}
      footer={mode.refused === 0 ? undefined : `${mode.refused} items refused`}
    >
      {mode.rows.map((row) => (
        <InsetRow key={row.group} accessibilityLabel={row.group}>
          <View className="flex-1 gap-2">
            <MonoText size="monoShell" tone="text_primary">
              {row.group}
            </MonoText>
            <MonoText tone="text_muted">
              {`n ${row.count} · first ${micros(row.firstMs)} · p50 ${micros(row.p50Ms)} · p95 ${micros(row.p95Ms)}`}
            </MonoText>
          </View>
        </InsetRow>
      ))}
    </InsetGroup>
  );
}
