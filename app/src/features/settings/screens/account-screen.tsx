// The limits of one account (AC-Limits 5 and 6), pushed from "Accounts on desktop" in
// Settings, with the account's sheet name as its title. One row per window or limit:
// its name, then "limit reached · resets 14:20" in state_error or "63% · resets Mon
// 09:00" in text_muted. The footer gives the age of the values in one form, "12m ago".
// No progress bars. The screen asks efrd for the list again when it opens.

import { useQuery } from "@tanstack/react-query";
import { Stack } from "expo-router";
import { ScrollView, Text } from "react-native";

import { cn } from "@/components/ui/cn";
import { InsetGroup, InsetRow } from "@/components/ui/inset-group";
import { MonoText } from "@/components/ui/mono-text";
import { modelsQueryOptions } from "@/features/conversation/conversation-queries";
import { useRuntime } from "@/features/navigation/use-app-services";
import { useMachines } from "@/features/navigation/use-machines";
import { useNow } from "@/features/navigation/use-now";
import { accountScreen } from "../model/account-limits";

export interface AccountScreenProps {
  /** The machine and the account of the route, or undefined when the params are bad. */
  readonly target: { readonly machine: string; readonly provider: string } | undefined;
}

/** The screen of one account. */
export function AccountScreen(props: AccountScreenProps) {
  const machines = useMachines();
  const target = props.target;
  const machine = machines.find((one) => one.id === target?.machine);
  if (target === undefined || machine === undefined) {
    return null;
  }
  return <AccountBody machine={machine.id} machineName={machine.name} provider={target.provider} />;
}

interface AccountBodyProps {
  readonly machine: string;
  readonly machineName: string;
  readonly provider: string;
}

function AccountBody(props: AccountBodyProps) {
  const runtime = useRuntime();
  const now = useNow();
  const list = useQuery({
    ...modelsQueryOptions(runtime, props.machine),
    refetchOnMount: "always",
  }).data;
  const view = accountScreen(list?.catalogs ?? [], props.provider, props.machineName, now);
  return (
    <ScrollView
      contentInsetAdjustmentBehavior="automatic"
      className="flex-1 bg-surface-1"
      contentContainerClassName="gap-26 px-inset pt-20 pb-20"
    >
      <Stack.Screen options={{ title: view?.title ?? "" }} />
      {view === undefined ? (
        <MonoText tone="text_muted" className="px-20">
          {`${props.machineName} names no such account`}
        </MonoText>
      ) : view.rows.length === 0 ? (
        <Text className="px-20 text-caption text-text-subtle">{view.footer}</Text>
      ) : (
        <InsetGroup header={view.header} footer={view.footer}>
          {view.rows.map((row) => (
            <InsetRow key={row.key} accessibilityLabel={row.label}>
              <Text numberOfLines={1} className="text-row flex-1 text-text-primary">
                {row.name}
              </Text>
              {/* AC-Limits 5 and 6: the value in mono 13, only "limit reached" in
                state_error, and the reset quiet after it. */}
              <Text
                numberOfLines={1}
                className={cn(
                  "text-mono-shell",
                  row.reached ? "text-state-error" : "text-text-muted",
                )}
              >
                {row.value}
                {row.reset === undefined ? null : (
                  <Text className="text-mono-shell text-text-subtle">{row.reset}</Text>
                )}
              </Text>
            </InsetRow>
          ))}
        </InsetGroup>
      )}
    </ScrollView>
  );
}
