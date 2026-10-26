// Settings (design 3.16): the paired machines, "Pair a new machine", the Haptics switch
// and the version, as an inset grouped list. It is pushed from the gear on any screen,
// and back returns to the screen under it untouched.
//
// - A machine row shows the state of its connection and the efrd version of its last
//   connection. Its menu copies the node name, opens Pairing for this machine, or forgets
//   it after the Forget sheet.
// - After a Forget, the row leaves the list with the system animation (none with Reduce
//   Motion). Then the stack follows the plan of model/forget-plan.ts: the last machine
//   resets the stack to the root with a cross-fade, which then shows pairing.
// - With no machine, the Machines group holds only "Pair a new machine".

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
import { use, type ReactNode } from "react";
import {
  Alert,
  LayoutAnimation,
  Pressable,
  ScrollView,
  Text,
  type HostInstance,
} from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { InsetGroup, InsetRow, SwitchRow } from "@/components/ui/inset-group";
import { MonoText } from "@/components/ui/mono-text";
import { showActionSheet } from "@/components/ui/native/show-action-sheet";
import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import { useReduceMotion } from "@/lib/native/reduce-motion";
import {
  forgetMachineMutationOptions,
  machineStatusQueryOptions,
} from "@/features/machines/machine-queries";
import type { Machine } from "@/features/machines/model/machine";
import { keylessConnection } from "@/features/machines/model/machine-connection";
import type { Runtime } from "@/features/machines/model/runtime";
import { MachineRow } from "../components/machine-row";
import { forgetPlan } from "../model/forget-plan";
import { DEFAULT_PREFERENCES } from "../model/preferences";
import {
  FORGET_ACTION,
  forgetFailedText,
  forgetSheet,
  machineRowText,
  nodeName,
  versionText,
  zoneAt,
  type MachineAction,
} from "../model/settings-format";
import { SettingsServicesContext, type SettingsServices } from "./settings-services";
import { useMinute } from "./use-minute";

/** The route of the dev gallery (wave 4), opened with a long press on the version. */
const GALLERY_ROUTE = "/gallery";

/** The connection of a machine whose supervisor is already gone (a Forget that runs). */
const NO_CONNECTION = createAtom(keylessConnection());

/** The version of the app, from the app config, such as "0.1.0 (14)". */
const APP_VERSION = versionText(
  Constants.expoConfig?.version,
  Constants.expoConfig?.ios?.buildNumber,
);

/** The settings screen. */
export function SettingsScreen() {
  const services = use(SettingsServicesContext);
  return services === null ? <UnwiredSettings /> : <WiredSettings services={services} />;
}

/** Settings before the layout gives the runtime: no machine, and a switch that keeps nothing. */
function UnwiredSettings() {
  const router = useRouter();
  return (
    <SettingsList
      machines={null}
      haptics={DEFAULT_PREFERENCES.haptics}
      onHapticsChange={() => undefined}
      onPair={() => router.push("/pair")}
    />
  );
}

interface WiredSettingsProps {
  readonly services: SettingsServices;
}

function WiredSettings(props: WiredSettingsProps) {
  const { runtime, preferences } = props.services;
  const router = useRouter();
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const reduceMotion = useReduceMotion();
  const registry = useSelector(runtime.registry);
  const haptics = useSelector(preferences.store, (state) => state.haptics);
  const now = useMinute();
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
      const text = forgetFailedText(machine.name);
      Alert.alert(text.title, text.message);
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

  return (
    <SettingsList
      machines={machines}
      haptics={haptics}
      onHapticsChange={(on) => preferences.setHaptics(on)}
      onPair={() => router.push("/pair")}
      onVersionLongPress={__DEV__ ? () => router.push(GALLERY_ROUTE) : undefined}
    />
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
  readonly haptics: boolean;
  readonly onHapticsChange: (on: boolean) => void;
  readonly onPair: () => void;
  readonly onVersionLongPress?: (() => void) | undefined;
}

/** The groups of Settings: Machines, Feedback and About. */
function SettingsList(props: SettingsListProps) {
  return (
    <ScrollView
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

/** The version of the app. In a dev build a long press opens the gallery of the screens. */
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
