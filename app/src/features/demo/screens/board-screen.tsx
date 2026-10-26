// The board route of the dev gallery (`/board?name=...`): a component board of
// model/boards.ts with the real screen or component in its state.
//
// - The pairing boards: the real pairing modal with the sample data of
//   features/pairing/model/pairing-boards.ts.
// - The no-machine root, Settings with no machine, and Settings with one machine (a
//   sandbox, for Forget the last machine).
// - The component sheets of the canvas: composer, approval block, transcript rows,
//   loaders and the icon.
// - The render counter: a conversation of the demo machine with its real header and
//   screen inside a probe, and the counts over it.
//
// The gallery imports the components and screens of the other features: it is the one
// place that shows them all (test/dependency-graph.test.ts allows it for this feature).

import { Redirect, Stack } from "expo-router";
import { useHeaderHeight } from "expo-router/react-navigation";
import { View } from "react-native";

import { MonoText } from "@/components/ui/mono-text";
import { DEV_TOOLS } from "@/lib/dev/dev-tools";
import { RenderProbe } from "@/lib/dev/render-probe";
import { SPACING } from "@/theme/metrics";
import { ConversationScreen } from "@/features/conversation/screens/conversation-screen";
import { ConversationHeader } from "@/features/navigation/components/screen-headers";
import { PAIR_OPTIONS, SETTINGS_OPTIONS } from "@/features/navigation/model/screen-options";
import { AppServicesContext } from "@/features/navigation/use-app-services";
import { pairingBoard } from "@/features/pairing/model/pairing-boards";
import { NoMachineRootScreen } from "@/features/pairing/screens/no-machine-root-screen";
import { PairingBoardScreen } from "@/features/pairing/screens/pairing-board-screen";
import { SettingsScreen } from "@/features/settings/screens/settings-screen";
import { ApprovalsSheet } from "../components/approvals-sheet";
import { ComposerSheet } from "../components/composer-sheet";
import { IconSheet } from "../components/icon-sheet";
import { LoadersSheet } from "../components/loaders-sheet";
import { RenderOverlay } from "../components/render-overlay";
import { SandboxSettings } from "../components/sandbox-settings";
import { TranscriptSheet } from "../components/transcript-sheet";
import type { BoardParams } from "../model/board-schemas";
import { galleryBoard, PAIRING_PREFIX, RENDER_BOARD } from "../model/boards";

/** The board screen. A build without the dev tools goes to the root. */
export function BoardScreen(props: BoardParams) {
  if (!DEV_TOOLS) {
    return <Redirect href="/" />;
  }
  return <Board {...props} />;
}

function Board(props: BoardParams) {
  const { name } = props;
  if (name.startsWith(PAIRING_PREFIX)) {
    const board = pairingBoard(name.slice(PAIRING_PREFIX.length));
    return board === undefined ? (
      <Missing name={name} />
    ) : (
      <>
        <Stack.Screen options={PAIR_OPTIONS} />
        <PairingBoardScreen board={board} />
      </>
    );
  }
  const title = galleryBoard(name)?.name ?? name;
  switch (name) {
    case "R-NoMachine":
      return <NoMachineRootScreen />;
    case "Settings-ForgetLast":
      return (
        <>
          <Stack.Screen options={SETTINGS_OPTIONS} />
          <SandboxSettings board={name} visit={props.visit ?? ""} machines={["desktop"]} />
        </>
      );
    case "Settings · no machine":
      return (
        <>
          <Stack.Screen options={SETTINGS_OPTIONS} />
          <AppServicesContext value={null}>
            <SettingsScreen />
          </AppServicesContext>
        </>
      );
    case RENDER_BOARD:
      return props.machine === undefined || props.id === undefined ? (
        <Missing name={name} />
      ) : (
        <RenderBoard machine={props.machine} id={props.id} />
      );
    default:
      return (
        <>
          <Stack.Screen options={{ title }} />
          <Sheet name={name} />
        </>
      );
  }
}

function Sheet(props: { readonly name: string }) {
  switch (props.name) {
    case "S-Composer":
      return <ComposerSheet />;
    case "S-Approvals":
      return <ApprovalsSheet />;
    case "S-Messages":
      return <TranscriptSheet kind="messages" />;
    case "S-Markdown":
      return <TranscriptSheet kind="markdown" />;
    case "S-Tools":
      return <TranscriptSheet kind="tools" />;
    case "S-Diff":
      return <TranscriptSheet kind="diff" expanded />;
    case "S-Loaders":
      return <LoadersSheet />;
    case "Icon":
      return <IconSheet />;
    default:
      return <Missing name={props.name} />;
  }
}

interface RenderBoardProps {
  readonly machine: string;
  readonly id: string;
}

/** A conversation with its real header, its screen inside the probe, and the counts. */
function RenderBoard(props: RenderBoardProps) {
  const top = useHeaderHeight() + SPACING.strip + 4;
  return (
    <View className="flex-1 bg-surface-1">
      <ConversationHeader conversation={{ machine: props.machine, id: props.id }} />
      <RenderProbe id="conversation">
        <ConversationScreen conversation={{ machine: props.machine, id: props.id }} />
      </RenderProbe>
      <RenderOverlay top={top} />
    </View>
  );
}

function Missing(props: { readonly name: string }) {
  return (
    <View className="flex-1 items-center justify-center bg-surface-1 px-gutter">
      <Stack.Screen options={{ title: "Gallery" }} />
      <MonoText tone="text_muted">{`no board ${props.name}`}</MonoText>
    </View>
  );
}
