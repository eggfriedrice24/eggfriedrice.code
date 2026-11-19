// The S-Composer board: the real docked composer in each state of the canvas
// (model/composer-cases.ts). A list of the states is above it; a tap on a state gives the
// composer the stores of that state, and it derives its look as on a real screen. The
// status line reads a stream that is idle or that runs a turn. Its calls never end. The
// composer rests as a one-line pill; a tap on it shows the full composer of the state.

import { createStore, type Store } from "@tanstack/store";
import { useState } from "react";
import { View } from "react-native";

import { InsetGroup, InsetRow } from "@/components/ui/inset-group";
import { SymbolIcon } from "@/components/ui/symbol";
import { DEFAULT_FACTS } from "@/lib/lock/owner-check";
import type { StreamState } from "@/lib/stream/reducer";
import { initialLock } from "@/features/navigation/model/app-lock";
import { DockedComposer } from "@/features/conversation/components/composer/composer";
import {
  COMPOSER_CASES,
  QUIET_CALLER,
  composerEnvironment,
  type ComposerCase,
  type ComposerEnvironment,
} from "../model/composer-cases";
import { demoConversation } from "../model/demo-conversations";
import { MemoryKeyValue } from "../model/demo-services";
import { runningTurnOf, staticState, SHEET_CONVERSATION } from "../model/static-stream";
import { SheetScroll } from "./sheet-layout";

/** The gallery has the Face ID unlock on, so Auto can be picked. It never asks iOS. */
const DEMO_LOCK = {
  store: createStore({
    ...initialLock({ on: true, notice: null }),
    open: true,
    shown: true,
    phase: "active" as const,
  }),
  facts: () => Promise.resolve(DEFAULT_FACTS),
};

/** The streams of the status line: an idle conversation and one with a running turn. */
interface SheetStreams {
  readonly idle: Store<StreamState>;
  readonly running: Store<StreamState>;
  readonly runningTurn: string | undefined;
}

function sheetStreams(): SheetStreams {
  let n = 0;
  const id = (): string => {
    n += 1;
    return `0199d3e0-de38-7000-8000-${n.toString(16).padStart(12, "0")}`;
  };
  const end = Date.now();
  const idle = demoConversation("notes", id);
  const streaming = demoConversation("streaming", id);
  // The streaming script up to the shell call: the turn runs and has its mode and cwd.
  const running = staticState([...streaming.history, ...streaming.live.slice(0, 20)], end);
  return {
    idle: createStore(staticState(idle.history, end)),
    running: createStore(running),
    runningTurn: runningTurnOf(running),
  };
}

/** The stores of `one`. */
function environmentOf(one: ComposerCase): ComposerEnvironment {
  let n = 0;
  return composerEnvironment(one, {
    storage: new MemoryKeyValue(),
    now: Date.now,
    newId: () => {
      n += 1;
      return `0199d3e0-de39-7000-8000-${n.toString(16).padStart(12, "0")}`;
    },
    conversation: SHEET_CONVERSATION,
  });
}

/** The S-Composer board. */
export function ComposerSheet() {
  const [streams] = useState(sheetStreams);
  const [environments] = useState(() => COMPOSER_CASES.map(environmentOf));
  const [selected, setSelected] = useState(0);
  const one = COMPOSER_CASES[selected];
  const env = environments[selected];
  if (one === undefined || env === undefined) {
    return null;
  }
  const runningTurnId = one.running ? streams.runningTurn : undefined;
  const props = {
    deps: {
      caller: QUIET_CALLER,
      outbox: env.outbox,
      drafts: env.drafts,
      preferences: env.preferences,
      modelChoices: env.modelChoices,
      newCommandId: () => "0199d3e0-de39-7000-8000-0000000000ff",
      // The gallery never asks iOS: each check passes.
      lock: DEMO_LOCK,
      attach: null,
      voice: null,
    },
    target: env.target,
    connection: env.connection,
    machineName: "desktop",
    tailnetDown: one.tailnetDown,
  };
  return (
    <View className="flex-1 bg-surface-1">
      <SheetScroll>
        <InsetGroup header="State" footer={one.hint ?? "Tap a state. The composer below shows it."}>
          {COMPOSER_CASES.map((each, index) => (
            <InsetRow
              key={each.name}
              title={each.name}
              accessory={
                index === selected ? (
                  <SymbolIcon name="checkmark" size={15} tone="accent_primary" label="Shown" />
                ) : null
              }
              onPress={() => setSelected(index)}
            />
          ))}
        </InsetGroup>
      </SheetScroll>
      {one.blankRoot ? (
        <DockedComposer key={one.name} {...props} />
      ) : runningTurnId === undefined ? (
        <DockedComposer key={one.name} {...props} stream={streams.idle} />
      ) : (
        <DockedComposer
          key={one.name}
          {...props}
          stream={streams.running}
          runningTurnId={runningTurnId}
        />
      )}
    </View>
  );
}
