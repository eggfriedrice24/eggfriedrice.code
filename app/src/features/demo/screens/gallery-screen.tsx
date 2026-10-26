// The dev gallery (plan section 0): every board of the design canvas and every undrawn
// state, by name, so the user can see each one on the iPhone without a paired machine.
// Settings > About opens it with a long press on the version, in a dev or preview build.
//
// A tap on a board pairs the demo machines first when the board needs them (the real
// pairing flow, with the demo server behind the socket), sets the stage of the board,
// and opens the real screen. The tools of W4-DEMO-MEASURE come last: the decode bench and
// the render counter.

import { Redirect, Stack, useRouter } from "expo-router";
import { useState } from "react";
import { View } from "react-native";

import { BodyText } from "@/components/ui/body-text";
import { BrailleCell } from "@/components/ui/braille-cell";
import { InsetGroup, InsetRow } from "@/components/ui/inset-group";
import { MonoText } from "@/components/ui/mono-text";
import { DEV_TOOLS } from "@/lib/dev/dev-tools";
import { SheetScroll } from "../components/sheet-layout";
import { SECTIONS, boardsOf, type GalleryBoard } from "../model/boards";
import { demoServer } from "../native/demo-machines";
import { useDemoMachines, useOpenRenderCounter, useVisitBoard } from "../use-gallery";

/** The gallery. A build without the dev tools goes to the root. */
export function GalleryScreen() {
  if (!DEV_TOOLS) {
    return <Redirect href="/" />;
  }
  return <Gallery />;
}

/** The words of an error of a visit. */
function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function Gallery() {
  const router = useRouter();
  const machines = useDemoMachines();
  const visit = useVisitBoard(demoServer);
  const openRenderCounter = useOpenRenderCounter(demoServer);
  // Short-lived state of this screen only: what runs now, and the last failure.
  const [busy, setBusy] = useState<string | undefined>(undefined);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const run = (name: string, action: () => Promise<void>): void => {
    if (busy !== undefined) {
      return;
    }
    setBusy(name);
    setFailure(undefined);
    action().then(
      () => setBusy(undefined),
      (error: unknown) => {
        setBusy(undefined);
        setFailure(`${name}: ${errorText(error)}`);
      },
    );
  };

  const allPaired = machines.paired === machines.total;
  return (
    <>
      <Stack.Screen options={{ title: "Gallery" }} />
      <SheetScroll>
        <InsetGroup
          header="Demo machines"
          footer="desktop, laptop and homelab play efrd from the app. They reach no computer."
        >
          <InsetRow title="Paired" value={`${machines.paired} of ${machines.total}`} />
          <InsetRow
            title={allPaired ? "Forget the demo machines" : "Pair the demo machines"}
            tone="accent_primary"
            onPress={() =>
              run(allPaired ? "Forget" : "Pair", allPaired ? machines.forgetAll : machines.pairAll)
            }
          />
        </InsetGroup>
        {failure === undefined ? null : (
          <View role="alert">
            <MonoText tone="state_error">{failure}</MonoText>
          </View>
        )}
        {SECTIONS.map(([section, title]) => (
          <InsetGroup key={section} header={title}>
            {boardsOf(section).map((board) => (
              <BoardRow
                key={board.name}
                board={board}
                busy={busy === board.name}
                onPress={() => run(board.name, () => visit(board))}
              />
            ))}
          </InsetGroup>
        ))}
        <InsetGroup
          header="Measure"
          footer="Run the bench alone, with the phone cool. React counts renders in a dev build only."
        >
          <InsetRow
            title="Decode bench (zod, jitless, hand)"
            onPress={() => router.push("/bench")}
          />
          <InsetRow
            title="Render counter on a fast reply"
            onPress={() => run("Render counter", openRenderCounter)}
          />
        </InsetGroup>
      </SheetScroll>
    </>
  );
}

interface BoardRowProps {
  readonly board: GalleryBoard;
  readonly busy: boolean;
  readonly onPress: () => void;
}

/** One board: its canvas name in mono, what it shows, and what to tap. */
function BoardRow(props: BoardRowProps) {
  const { board } = props;
  return (
    <InsetRow accessibilityLabel={`${board.name}: ${board.title}`} onPress={props.onPress}>
      <View className="flex-1 gap-2">
        <MonoText size="monoShell" tone="text_bright">
          {board.name}
        </MonoText>
        <BodyText size="caption" tone="text_muted">
          {board.title}
        </BodyText>
        {board.hint === undefined ? null : (
          <BodyText size="caption" tone="text_subtle">
            {board.hint}
          </BodyText>
        )}
      </View>
      {props.busy ? <BrailleCell tint="wait" label="Opening" /> : null}
    </InsetRow>
  );
}
