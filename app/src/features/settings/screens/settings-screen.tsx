// Settings (design 3.16 and the Settings board): the paired machines, "Pair a new
// machine", the switch "Ask before a costly model switch", the Haptics switch and the
// version, as an inset grouped list. It is pushed from the gear on any screen, and back
// returns to the screen under it untouched.
//
// - A machine row shows the state of its connection and the efrd version of its last
//   connection. Its menu copies the node name, opens Pairing for this machine, or forgets
//   it after the Forget sheet.
// - After a Forget, the row leaves the list with the system animation (none with Reduce
//   Motion). Then the stack follows the plan of model/forget-plan.ts: the last machine
//   resets the stack to the root with a cross-fade, which then shows pairing.
// - With no machine, the Machines group holds only "Pair a new machine".
// - The group Privacy holds the row "Face ID unlock" (round 9). A tap asks iOS first; the
//   switch moves only after the check passed. With `focus=unlock` (the item "Turn on Face
//   ID unlock" of the mode menu) Settings scrolls to the group and marks the row with
//   selection_bg for about 1 s.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useSelector } from "@tanstack/react-store";
import { createAtom } from "@tanstack/store";
import Constants from "expo-constants";
import { useRouter } from "expo-router";
import {
  useNavigation,
  type NavigationProp,
  type ParamListBase,
} from "expo-router/react-navigation";
import { use, useRef, useState, type ReactNode } from "react";
import {
  LayoutAnimation,
  Pressable,
  ScrollView,
  Text,
  View,
  type HostInstance,
  type LayoutChangeEvent,
  type ScrollViewInstance,
} from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { InsetGroup, InsetRow, SwitchRow } from "@/components/ui/inset-group";
import { MonoText } from "@/components/ui/mono-text";
import { showActionSheet } from "@/components/ui/native/show-action-sheet";
import { DEV_TOOLS } from "@/lib/dev/dev-tools";
import { DEFAULT_FACTS } from "@/lib/lock/owner-check";
import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import { useReduceMotion } from "@/lib/native/reduce-motion";
import {
  forgetMachineMutationOptions,
  machineStatusQueryOptions,
} from "@/features/machines/machine-queries";
import type { Machine } from "@/features/machines/model/machine";
import { keylessConnection } from "@/features/machines/model/machine-connection";
import { zoneAt } from "@/features/machines/model/machine-status";
import type { Runtime } from "@/features/machines/model/runtime";
import {
  type AppLock,
  selectChecking,
  selectLockOn,
  selectNotice,
} from "@/features/navigation/model/app-lock";
import {
  noPasscodeNotice,
  unlockName,
  unlockRowLine,
} from "@/features/navigation/model/owner-check-format";
import { ownerFactsQueryOptions } from "@/features/navigation/navigation-queries";
import { AppServicesContext } from "@/features/navigation/use-app-services";
import { useNow } from "@/features/navigation/use-now";
import { accountHref } from "@/features/navigation/model/routes";
import { AccountsGroup } from "../components/accounts-group";
import { UnlockRow } from "../components/unlock-row";
import { MachineRow } from "../components/machine-row";
import { forgetPlan } from "../model/forget-plan";
import { DEFAULT_PREFERENCES, type PreferencesStore } from "../model/preferences";
import {
  FORGET_ACTION,
  forgetFailedText,
  forgetSheet,
  machineRowText,
  nodeName,
  versionText,
  type MachineAction,
} from "../model/settings-format";

/** The route of the dev gallery (wave 4), opened with a long press on the version. */
const GALLERY_ROUTE = "/gallery";

/** The connection of a machine whose supervisor is already gone (a Forget that runs). */
const NO_CONNECTION = createAtom(keylessConnection());

/** The version of the app, from the app config, such as "0.1.0 (14)". */
const APP_VERSION = versionText(
  Constants.expoConfig?.version,
  Constants.expoConfig?.ios?.buildNumber,
);

/** How long the row "Face ID unlock" stays marked after `focus=unlock` (about 1 s). */
export const UNLOCK_MARK_MS = 1000;

export interface SettingsScreenProps {
  /** `unlock`: scroll to the row "Face ID unlock" and mark it (S-ComposerR7 r9 12b). */
  readonly focus?: "unlock" | undefined;
}

/** The settings screen. */
export function SettingsScreen(props: SettingsScreenProps) {
  const services = use(AppServicesContext);
  return services === null ? (
    <UnwiredSettings />
  ) : (
    <WiredSettings
      runtime={services.runtime}
      preferences={services.preferences}
      lock={services.appLock}
      focus={props.focus}
    />
  );
}

/** Settings without the services of the root layout: no machine, and a switch that keeps nothing. */
function UnwiredSettings() {
  const router = useRouter();
  return (
    <SettingsList
      machines={null}
      haptics={DEFAULT_PREFERENCES.haptics}
      onHapticsChange={() => undefined}
      askCostly={DEFAULT_PREFERENCES.askCostlySwitch}
      onAskCostlyChange={() => undefined}
      onPair={() => router.push("/pair")}
    />
  );
}

interface WiredSettingsProps {
  readonly runtime: Runtime;
  readonly preferences: PreferencesStore;
  readonly lock: AppLock;
  readonly focus: "unlock" | undefined;
}

function WiredSettings(props: WiredSettingsProps) {
  const { runtime, preferences } = props;
  const router = useRouter();
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const reduceMotion = useReduceMotion();
  const registry = useSelector(runtime.registry);
  const haptics = useSelector(preferences.store, (state) => state.haptics);
  const askCostly = useSelector(preferences.store, (state) => state.askCostlySwitch);
  const now = useNow();
  const forget = useMutation(forgetMachineMutationOptions(runtime));

  /** Applies the stack plan after machine `machine` is forgotten. */
  const followForget = (machine: Machine): void => {
    const state = navigation.getState();
    const plan = forgetPlan(state.routes, machine.id, runtime.registry.get().machines.length);
    if (plan.kind === "stay") {
      return;
    }
    if (plan.fade) {
      navigation.setOptions({ animation: reduceMotion ? "none" : "fade" });
    }
    const routes = state.routes.filter((_, index) => plan.keep.includes(index));
    navigation.reset({ ...state, index: routes.length - 1, routes, stale: false });
  };

  const confirmForget = async (machine: Machine, anchor: HostInstance | null): Promise<void> => {
    const last = registry.machines.length <= 1;
    const answer = await showActionSheet(forgetSheet(machine.name, last), anchor);
    if (answer !== FORGET_ACTION) {
      return;
    }
    if (!reduceMotion && !last) {
      LayoutAnimation.configureNext(LayoutAnimation.Presets.easeInEaseOut);
    }
    let failed = false;
    try {
      await forget.mutateAsync(machine.id);
    } catch {
      // The machine is gone from the list even so; only its key stays on the phone.
      failed = true;
    }
    followForget(machine);
    if (failed) {
      // An action sheet and not an alert: it stays under the lock screen (lib/native/links.ts).
      const text = forgetFailedText(machine.name);
      void showActionSheet({ ...text, actions: [], cancelLabel: "OK" });
    }
  };

  const onAction = (machine: Machine, action: MachineAction, anchor: HostInstance | null): void => {
    switch (action) {
      case "copy":
        void copyText(nodeName(machine));
        haptic("light");
        return;
      case "repair":
        router.push({ pathname: "/pair", params: { machine: machine.id } });
        return;
      default:
        void confirmForget(machine, anchor).catch(() => undefined);
    }
  };

  const machines = registry.loaded ? (
    registry.machines.map((machine) => (
      <ConnectedMachineRow
        key={machine.id}
        runtime={runtime}
        machine={machine}
        now={now}
        onAction={(action, anchor) => onAction(machine, action, anchor)}
      />
    ))
  ) : (
    <LoadingRow key="loading" />
  );

  const accounts = registry.machines.map((machine) => (
    <AccountsGroup
      key={machine.id}
      caller={runtime}
      machine={machine}
      now={now}
      onOpen={(provider) => router.push(accountHref(machine.id, provider))}
    />
  ));

  return (
    <SettingsList
      machines={machines}
      accounts={accounts}
      unlock={<UnlockGroup lock={props.lock} mark={props.focus === "unlock"} />}
      focusUnlock={props.focus === "unlock"}
      reduceMotion={reduceMotion}
      haptics={haptics}
      onHapticsChange={(on) => preferences.setHaptics(on)}
      askCostly={askCostly}
      onAskCostlyChange={(on) => preferences.setAskCostlySwitch(on)}
      onPair={() => router.push("/pair")}
      onVersionLongPress={DEV_TOOLS ? () => router.push(GALLERY_ROUTE) : undefined}
    />
  );
}

interface UnlockGroupProps {
  readonly lock: AppLock;
  /** Mark the row for about 1 s when it shows (`focus=unlock`). */
  readonly mark: boolean;
}

/** The group Privacy: the row "Face ID unlock" with the state of the lock. */
function UnlockGroup(props: UnlockGroupProps) {
  const { lock } = props;
  const [marked, setMarked] = useState(props.mark);
  const started = useRef(false);
  // NOTE: the mark goes after a fixed time from its first layout. A timer and not an
  // animation, so Reduce Motion gets the same still frame.
  const onLayout = (): void => {
    if (marked && !started.current) {
      started.current = true;
      setTimeout(() => setMarked(false), UNLOCK_MARK_MS);
    }
  };
  const on = useSelector(lock.store, selectLockOn);
  const checking = useSelector(lock.store, (state) => {
    const purpose = selectChecking(state);
    return purpose === "turn_on" || purpose === "turn_off";
  });
  const notice = useSelector(lock.store, selectNotice);
  const facts = useQuery(ownerFactsQueryOptions(lock)).data ?? DEFAULT_FACTS;
  const disabled = !facts.passcode && !on;
  // The notice of frame 6 stays only while the iPhone has no passcode. When a passcode
  // is set again, the row is frame 1 again.
  const noticed = notice === "no_passcode" && disabled;
  return (
    <InsetGroup header="Privacy">
      <UnlockRow
        title={unlockName(facts.method)}
        line={noticed ? noPasscodeNotice(facts.method) : unlockRowLine(facts)}
        notice={noticed}
        marked={marked}
        onLayout={onLayout}
        value={on}
        checking={checking}
        disabled={disabled}
        onValueChange={(next) => lock.setOn(next)}
      />
    </InsetGroup>
  );
}

interface ConnectedMachineRowProps {
  readonly runtime: Runtime;
  readonly machine: Machine;
  readonly now: number;
  readonly onAction: (action: MachineAction, anchor: HostInstance | null) => void;
}

/** A machine row that follows the connection of its machine and its kept status. */
function ConnectedMachineRow(props: ConnectedMachineRowProps) {
  const { runtime, machine, now } = props;
  const connection = useSelector(runtime.machine(machine.id)?.connection ?? NO_CONNECTION);
  const status = useQuery(machineStatusQueryOptions(runtime, machine.id)).data;
  return (
    <MachineRow
      text={machineRowText(machine, connection, status, now, zoneAt(now))}
      onAction={props.onAction}
    />
  );
}

/** The row of the Machines group while the app still reads the pairing records. */
function LoadingRow() {
  return (
    <InsetRow accessibilityLabel="Loading the machines">
      <BrailleCell tint="wait" />
      <MonoText tone="text_muted">loading the machines</MonoText>
    </InsetRow>
  );
}

interface SettingsListProps {
  /** The rows of the paired machines, or null for none. */
  readonly machines: ReactNode;
  /** The groups "Accounts on <machine>", one per machine whose list the app has. */
  readonly accounts?: ReactNode;
  /** The group Privacy with the row "Face ID unlock"; none without the services. */
  readonly unlock?: ReactNode;
  readonly haptics: boolean;
  readonly onHapticsChange: (on: boolean) => void;
  /** "Ask before a costly model switch". */
  readonly askCostly: boolean;
  readonly onAskCostlyChange: (on: boolean) => void;
  readonly onPair: () => void;
  readonly onVersionLongPress?: (() => void) | undefined;
  /** Scroll to the group Privacy once it has its place (`focus=unlock`). */
  readonly focusUnlock?: boolean;
  readonly reduceMotion?: boolean;
}

/** The space above the group Privacy after the scroll to it. */
const FOCUS_MARGIN = 20;

/** The groups of Settings: Machines, the accounts of each machine, Models, Privacy,
 * Feedback, About. */
function SettingsList(props: SettingsListProps) {
  const scroll = useRef<ScrollViewInstance>(null);
  const scrolled = useRef(false);
  const onUnlockLayout = (event: LayoutChangeEvent): void => {
    if (props.focusUnlock !== true || scrolled.current) {
      return;
    }
    scrolled.current = true;
    const y = Math.max(0, event.nativeEvent.layout.y - FOCUS_MARGIN);
    scroll.current?.scrollTo({ y, animated: props.reduceMotion !== true });
  };
  return (
    <ScrollView
      ref={scroll}
      testID="settings-list"
      contentInsetAdjustmentBehavior="automatic"
      className="flex-1 bg-surface-1"
      contentContainerClassName="gap-26 px-inset pt-20 pb-20"
    >
      <InsetGroup
        header="Machines"
        footer={
          <>
            {"Run "}
            <Text className="text-mono-xs text-text-muted">efr device add</Text>
            {" on the machine, then scan its code."}
          </>
        }
      >
        {props.machines}
        <InsetRow
          key="pair"
          title="Pair a new machine"
          tone="accent_primary"
          symbol="plus"
          onPress={props.onPair}
        />
      </InsetGroup>
      {props.accounts}
      <InsetGroup
        header="Models"
        footer="A switch that compacts the conversation waits for you to confirm. Off: picking a model switches at once."
      >
        <SwitchRow
          title="Ask before a costly model switch"
          value={props.askCostly}
          onValueChange={props.onAskCostlyChange}
        />
      </InsetGroup>
      {props.unlock === undefined ? null : (
        <View testID="settings-privacy" onLayout={onUnlockLayout}>
          {props.unlock}
        </View>
      )}
      <InsetGroup header="Feedback" footer="Haptics for taps, copies, scans and errors.">
        <SwitchRow title="Haptics" value={props.haptics} onValueChange={props.onHapticsChange} />
      </InsetGroup>
      <InsetGroup header="About">
        <VersionRow onLongPress={props.onVersionLongPress} />
      </InsetGroup>
    </ScrollView>
  );
}

interface VersionRowProps {
  readonly onLongPress: (() => void) | undefined;
}

/**
 * The version of the app. In a dev or preview build (the dev tools) a long press opens
 * the gallery of the screens.
 */
function VersionRow(props: VersionRowProps) {
  const row = <InsetRow title="Version" value={APP_VERSION} valueMono />;
  const onLongPress = props.onLongPress;
  if (onLongPress === undefined) {
    return row;
  }
  return (
    <Pressable onLongPress={onLongPress} testID="version-row">
      {row}
    </Pressable>
  );
}
