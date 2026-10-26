// The root of the stack: a blank conversation on the machine of the root (plan section
// 2.5, design 3.2 and board Main).
//
// - The shell block under the bar: the machine, the cwd `~` and the mode, then the yolk ❯
//   with the blinking cursor (solid with Reduce Motion).
// - The composer at the bottom, the same glass pill as in a conversation. Its status line
//   shows the cwd and the mode of a new conversation. The first send puts the prompt in the outbox for a new conversation and pushes that
//   conversation with no animation. The root then has an empty draft and a new local id,
//   so it stays blank under the new screen.
// - A pick in the machine menu changes the machine of the root. The body follows it at
//   the priority of a transition (`useDeferredValue`): a store update cannot run in a
//   transition itself (plan section 2.4, "uSES rule").
// - With no machine paired, the root is the pairing intro (design 3.18). The route gives
//   it as `noMachine`, because a feature does not import the screens of another one.
//
// The header belongs to the root stack (features/navigation), so this screen sets none.

import { useHeaderHeight } from "expo-router/react-navigation";
import { useDeferredValue, useState, type ReactNode } from "react";
import { Keyboard, Pressable, Text, View } from "react-native";

import { BlockCursor } from "@/components/ui/block-cursor";
import { MonoText } from "@/components/ui/mono-text";
import { newCommandId } from "@/lib/native/database";
import type { Machine } from "@/features/machines/model/machine";
import { isTailnetDown } from "@/features/machines/model/machine-status";
import {
  useDrafts,
  useOutbox,
  usePreferences,
  useRuntime,
} from "@/features/navigation/use-app-services";
import { useMachineConnections, useRootMachine } from "@/features/navigation/use-machines";
import { useOpenConversation } from "@/features/navigation/use-open-conversation";
import { DockedComposer } from "../components/composer/composer";
import { BLANK_CWD, BLANK_MODE, type ComposerTarget } from "../model/composer-state";
import type { Outbox, OutboxEntry } from "../model/outbox";
import type { ComposerDeps } from "../use-composer-actions";

/** The space between the bottom of the bar and the shell block (top 128 on the canvas). */
export const SHELL_TOP = 22;

export interface BlankRootScreenProps {
  /** The root when no machine is paired: the pairing intro. */
  readonly noMachine: ReactNode;
}

/** The blank root screen. */
export function BlankRootScreen(props: BlankRootScreenProps) {
  const current = useRootMachine();
  const machine = useDeferredValue(current);
  if (current === undefined || machine === undefined) {
    return props.noMachine;
  }
  return <BlankRoot machine={machine} />;
}

interface BlankRootProps {
  readonly machine: Machine;
}

/** A new local id for the next conversation of the root. */
function newLocalId(outbox: Outbox): string {
  const target = outbox.newConversation();
  return target.kind === "new" ? target.localId : target.conversationId;
}

function BlankRoot(props: BlankRootProps) {
  const { machine } = props;
  const runtime = useRuntime();
  const outbox = useOutbox();
  const drafts = useDrafts();
  const preferences = usePreferences();
  const open = useOpenConversation();
  const top = useHeaderHeight();
  const connections = useMachineConnections();
  const [localId, setLocalId] = useState(() => newLocalId(outbox));
  const supervisor = runtime.machine(machine.id);
  const deps: ComposerDeps = { caller: runtime, outbox, drafts, preferences, newCommandId };
  const target: ComposerTarget = { machine: machine.id, id: localId, isBlankRoot: true };
  const onSend = (entry: OutboxEntry): void => {
    if (entry.target.kind !== "new") {
      return;
    }
    open({ machine: entry.machine, id: entry.target.localId }, { firstSend: true });
    setLocalId(newLocalId(outbox));
  };
  return (
    // A tap on the empty screen puts the keyboard away, as in any iOS text screen. The
    // composer sits on top, so its own taps never reach this.
    <Pressable
      accessible={false}
      onPress={() => Keyboard.dismiss()}
      className="flex-1 bg-surface-1"
    >
      <ShellBlock top={top + SHELL_TOP} machineName={machine.name} />
      {supervisor === undefined ? null : (
        // The wrapper fills the screen for the same reason as in conversation-screen.tsx:
        // a shorter one would drop the taps on the composer that the keyboard moved up.
        <View pointerEvents="box-none" className="absolute inset-0 justify-end">
          <DockedComposer
            deps={deps}
            target={target}
            connection={supervisor.connection}
            machineName={machine.name}
            tailnetDown={isTailnetDown([...connections.values()])}
            onSend={onSend}
          />
        </View>
      )}
    </Pressable>
  );
}

interface ShellBlockProps {
  /** The distance from the top of the screen. */
  readonly top: number;
  readonly machineName: string;
}

/** The two lines of the shell block (design 3.2): mono 13/19 at the gutter. */
function ShellBlock(props: ShellBlockProps) {
  return (
    <View className="px-gutter" style={{ paddingTop: props.top }}>
      <MonoText
        size="monoShell"
        tone="text_subtle"
        accessibilityLabel={`${props.machineName}, in ${BLANK_CWD}, ${BLANK_MODE}`}
      >
        <MonoText size="monoShell" tone="text_bright">
          {props.machineName}
        </MonoText>{" "}
        <MonoText size="monoShell" tone="prompt_directory">
          {BLANK_CWD}
        </MonoText>
        {` ${BLANK_MODE}`}
      </MonoText>
      <View className="flex-row items-center" accessibilityElementsHidden>
        <Text className="text-mono-shell text-accent-primary">❯ </Text>
        <BlockCursor size="shell" mode="blink" />
      </View>
    </View>
  );
}
