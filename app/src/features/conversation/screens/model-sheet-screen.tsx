// The model sheet route (S-ModelSheet, MS-*, AC-Out 5, AC-Limits 1-2): the root stack
// presents it as a form sheet that fits its content, with the grabber and no bar
// (MODEL_OPTIONS). It opens from the model button of a composer, and from a switch
// action under a turn that failed because its account is out (with that model selected).
//
// - The sheet asks efrd for the list again each time it opens, and shows the last list
//   at once.
// - It holds the stream of its conversation, for the model of the conversation and its
//   running turn.
// - A pick selects a row; the check moves only on confirm, which stores the choice of
//   the composer (model/model-choices.ts) and closes the sheet. The next message carries
//   it.
// - While the phone asks before a costly switch (Settings) and "Don't ask again here" was
//   off when the sheet opened, the sheet asks efrd what the switch costs
//   (`conversation.switch_preview`). Only a compaction shows. With the Settings switch
//   off, a tap on a row switches at once and closes the sheet, and a move of the slider
//   switches the effort at once.
// - Round 7, native after the device check (the hard rule of app/AGENTS.md, as the model
//   sheet of t3code): the native header says "Model" and has "Use" (or "Switch" for a
//   switch that compacts) at its right, off until something changed. The native search
//   bar and the filter menu (the provider, then "Legacy models") are in the bottom
//   toolbar of iOS 26. The search and the provider live while the sheet is open; the
//   provider goes back to All when it closes. "Legacy models" is a preference of the
//   phone. While the search has the focus or text, the sheet holds the full height. A
//   star sets the favourite of its account at once; the rows keep the order of the time
//   when the sheet opened.

import { useQuery } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";
import { createStore, shallow } from "@tanstack/store";
import { Stack, useRouter } from "expo-router";
import { useState } from "react";

import { haptic } from "@/lib/native/haptics";
import { EMPTY_STREAM, runningTurn } from "@/lib/stream/reducer";
import type { MachineId } from "@/features/machines/model/machine-ref";
import {
  useModelChoices,
  useOutbox,
  usePreferences,
  useRuntime,
} from "@/features/navigation/use-app-services";
import { useMachines } from "@/features/navigation/use-machines";
import { useNow } from "@/features/navigation/use-now";
import { ModelSheetView, type SheetEmpty } from "../components/model-sheet";
import { modelsQueryOptions, switchPreviewQueryOptions } from "../conversation-queries";
import { composerDraftKey, outboxTargetOf } from "../model/composer-state";
import {
  favouriteOf,
  selectFavourites,
  selectModelChoice,
  selectPlan,
  selectQuiet,
} from "../model/model-choices";
import {
  choiceOf,
  currentSettings,
  defaultSelection,
  freshChoice,
  type KindFilter,
  openingSelection,
  pickRow,
  type Selection,
  selectionChanged,
  type SheetRow,
  sheetView,
} from "../model/model-sheet";
import { selectLastSettings } from "../model/selectors";
import { useSheetStream } from "./use-sheet-stream";

/** The params of the route. */
export interface ModelSheetTargetParams {
  readonly machine: MachineId;
  readonly id: string;
  readonly root?: "1";
  readonly model?: string;
  readonly provider?: string;
}

export interface ModelSheetScreenProps {
  /** The composer of the route, or undefined when the params are bad. */
  readonly target: ModelSheetTargetParams | undefined;
}

/** The stream of a sheet without a conversation: no turn. */
const NO_STREAM = createStore(EMPTY_STREAM);

/** The model sheet. */
export function ModelSheetScreen(props: ModelSheetScreenProps) {
  const machines = useMachines();
  const target = props.target;
  const machine = machines.find((one) => one.id === target?.machine);
  if (target === undefined || machine === undefined) {
    return null;
  }
  return <SheetBody target={target} machineName={machine.name} />;
}

interface SheetBodyProps {
  readonly target: ModelSheetTargetParams;
  readonly machineName: string;
}

function SheetBody(props: SheetBodyProps) {
  const { target, machineName } = props;
  const runtime = useRuntime();
  const choices = useModelChoices();
  const preferences = usePreferences();
  const outbox = useOutbox();
  const router = useRouter();
  const now = useNow();
  const composer = { machine: target.machine, id: target.id, isBlankRoot: target.root === "1" };
  const key = composerDraftKey(composer);
  const conversationId = useSelector(outbox.store, (snapshot) => {
    if (composer.isBlankRoot) {
      return undefined;
    }
    const to = outboxTargetOf(snapshot, composer);
    return to.kind === "conversation" ? to.conversationId : undefined;
  });
  const stream = useSheetStream(
    conversationId === undefined ? undefined : { machine: target.machine, id: conversationId },
  );
  const store = stream?.store ?? NO_STREAM;
  const last = useSelector(store, selectLastSettings, { compare: shallow });
  const running = useSelector(store, (state) => runningTurn(state) !== undefined);
  const models = useQuery({
    ...modelsQueryOptions(runtime, target.machine),
    refetchOnMount: "always",
  });
  const list = models.data;
  const choice = useSelector(choices.store, selectModelChoice(key));
  const quiet = useSelector(choices.store, selectQuiet(key));
  const askCostly = useSelector(preferences.store, (state) => state.askCostlySwitch);
  // "Don't ask again here" counts from the next time the sheet opens, so the switch of
  // this time keeps its warning when the user ticks it.
  const [quietAtOpen] = useState(() => choices.quiet(key));
  const [picked, setPicked] = useState<Selection | undefined>(undefined);
  const showLegacy = useSelector(preferences.store, (state) => state.showLegacyModels);
  const favourites = useSelector(choices.store, selectFavourites);
  const plan = useSelector(choices.store, selectPlan(target.machine));
  // The rows keep the order of the favourites of the time when the sheet opened.
  const [favouritesAtOpen] = useState(() => choices.store.get().favourites);
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<KindFilter>("all");
  const [focused, setFocused] = useState(false);

  const favourite = (provider: string | undefined): string | undefined =>
    favouriteOf(favourites, target.machine, provider);
  const fresh = composer.isBlankRoot ? freshChoice(list, plan, favourite) : undefined;
  const current = currentSettings(list, choice, last.model === undefined ? undefined : last, fresh);
  const preselect =
    target.model === undefined ? undefined : { model: target.model, provider: target.provider };
  const selection = picked ?? openingSelection(list, current, preselect);
  const asks = askCostly && !quietAtOpen;
  const previewOn =
    asks &&
    conversationId !== undefined &&
    selection !== undefined &&
    selectionChanged(current, selection);
  const preview = useQuery({
    ...switchPreviewQueryOptions(
      runtime,
      { machine: target.machine, id: conversationId ?? "" },
      selection?.model ?? "",
      selection?.provider,
    ),
    enabled: previewOn,
  });

  const view = sheetView({
    list,
    current,
    selection,
    machineName,
    now,
    running,
    asks,
    cost: previewOn ? preview.data?.cost : undefined,
    costPending: previewOn && preview.isPending && preview.fetchStatus === "fetching",
    query,
    kind,
    showLegacy,
    favourite,
    favouriteAtOpen: (provider) => favouriteOf(favouritesAtOpen, target.machine, provider),
  });
  const searching = focused || query !== "";
  let empty: SheetEmpty;
  if (list === undefined) {
    empty = models.isError ? "offline" : "loading";
  }

  const close = (): void => {
    if (router.canGoBack()) {
      router.back();
    }
  };
  const commit = (next: Selection): void => {
    choices.set(key, choiceOf(next));
    haptic("light");
    close();
  };
  const select = (next: Selection | undefined): void => {
    if (next === undefined) {
      return;
    }
    if (askCostly) {
      setPicked(next);
    } else {
      commit(next);
    }
  };
  const onRow = (row: SheetRow): void => {
    if (!row.disabled) {
      select(pickRow(list, current, row.model, row.provider));
    }
  };
  const onEffort = (effort: string): void => {
    if (selection === undefined) {
      return;
    }
    const next = { ...selection, effort };
    if (askCostly) {
      setPicked(next);
    } else {
      // The switch of the Settings is off: the effort changes at once, the sheet stays.
      choices.set(key, choiceOf(next));
      setPicked(undefined);
    }
  };
  const onStar = (row: SheetRow): void => {
    choices.setFavourite(target.machine, row.provider, row.favourite ? undefined : row.model);
    haptic("light");
  };
  const { filter, confirm } = view;
  const kinds: [KindFilter, string][] = [
    ["all", "All providers"],
    ["claude", "Claude"],
    ["openai", "OpenAI"],
  ];
  return (
    <>
      <Stack.Screen options={{ sheetAllowedDetents: searching ? [1] : [0.75, 1] }} />
      {/* The native search bar of iOS, in the bottom toolbar (the hard rule of
          app/AGENTS.md: native parts over JS copies). */}
      <Stack.SearchBar
        placeholder={filter.placeholder}
        autoCapitalize="none"
        hideNavigationBar={false}
        obscureBackground={false}
        allowToolbarIntegration
        onChangeText={(event) => setQuery(event.nativeEvent.text)}
        onCancelButtonPress={() => setQuery("")}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
      />
      <Stack.Toolbar placement="bottom">
        <Stack.Toolbar.SearchBarSlot />
        <Stack.Toolbar.Menu
          accessibilityLabel={filter.label}
          icon={
            filter.on
              ? "line.3.horizontal.decrease.circle.fill"
              : "line.3.horizontal.decrease.circle"
          }
          separateBackground
          title="Model filters"
        >
          {filter.kinds ? (
            <Stack.Toolbar.Menu inline title="Provider">
              {kinds.map(([option, title]) => (
                <Stack.Toolbar.MenuAction
                  key={option}
                  isOn={filter.kind === option}
                  onPress={() => setKind(option)}
                >
                  {title}
                </Stack.Toolbar.MenuAction>
              ))}
            </Stack.Toolbar.Menu>
          ) : null}
          <Stack.Toolbar.MenuAction
            isOn={filter.legacy}
            onPress={() => preferences.setShowLegacyModels(!filter.legacy)}
          >
            Legacy models
          </Stack.Toolbar.MenuAction>
        </Stack.Toolbar.Menu>
      </Stack.Toolbar>
      <Stack.Toolbar placement="right">
        <Stack.Toolbar.Button
          accessibilityLabel={confirm?.label ?? "Nothing changed"}
          disabled={confirm === undefined || confirm.pending}
          variant="done"
          onPress={() => {
            if (selection !== undefined && confirm !== undefined) {
              commit(selection);
            }
          }}
        >
          {confirm?.compacts === true ? "Switch" : "Use"}
        </Stack.Toolbar.Button>
      </Stack.Toolbar>
      <ModelSheetView
        view={view}
        empty={empty}
        machineName={machineName}
        quiet={quiet}
        onStar={onStar}
        onRow={onRow}
        onEffort={onEffort}
        onUseDefault={() => select(defaultSelection(list))}
        onQuiet={() => choices.setQuiet(key, !quiet)}
      />
    </>
  );
}
