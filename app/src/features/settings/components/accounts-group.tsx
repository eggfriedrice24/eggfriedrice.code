// The group "Accounts on desktop" of Settings (AC-Limits 4): one row per account of the
// machine with its fullest limit, in the order of `models.list`. A row pushes the screen
// of that account. The group shows once the app has the list of the machine.

import { useQuery } from "@tanstack/react-query";
import { Text, View } from "react-native";

import { InsetGroup, InsetRow } from "@/components/ui/inset-group";
import { SymbolIcon } from "@/components/ui/symbol";
import type { MachineCaller } from "@/lib/query/machine-caller";
import { modelsQueryOptions } from "@/features/conversation/conversation-queries";
import type { Machine } from "@/features/machines/model/machine";
import { accountRows, type AccountRow } from "../model/account-limits";

export interface AccountsGroupProps {
  readonly caller: MachineCaller;
  readonly machine: Machine;
  readonly now: number;
  readonly onOpen: (provider: string) => void;
}

/** The accounts of one machine, or nothing before its list came. */
export function AccountsGroup(props: AccountsGroupProps) {
  const { machine } = props;
  const list = useQuery(modelsQueryOptions(props.caller, machine.id)).data;
  const rows = accountRows(list?.catalogs ?? [], machine.name, props.now);
  if (rows.length === 0) {
    return null;
  }
  return (
    <InsetGroup
      header={`Accounts on ${machine.name}`}
      footer="The fullest limit of each account, from its last call. An API key shows a limit only when it was hit."
    >
      {rows.map((row) => (
        <InsetRow
          key={row.provider}
          accessibilityLabel={row.label}
          onPress={() => props.onOpen(row.provider)}
        >
          <AccountRowContent row={row} />
        </InsetRow>
      ))}
    </InsetGroup>
  );
}

function AccountRowContent(props: { readonly row: AccountRow }) {
  const { row } = props;
  const value = row.value;
  return (
    <>
      <View className="flex-1">
        <Text numberOfLines={1} className="text-row text-text-primary">
          {row.name}
        </Text>
        {row.note === undefined ? null : (
          <Text numberOfLines={1} className="text-caption text-text-subtle">
            {row.note}
          </Text>
        )}
      </View>
      {value.kind === "warn" ? (
        <View className="flex-row items-center gap-4">
          <SymbolIcon name="exclamationmark.triangle" size={13} tone="state_warning" />
          <Text className="text-mono-shell text-text-subtle">{value.text}</Text>
        </View>
      ) : null}
      {value.kind === "reached" ? (
        <Text className="text-row text-state-error">{value.text}</Text>
      ) : null}
      <SymbolIcon name="chevron.right" size={13} tone="text_subtle" weight="semibold" />
    </>
  );
}
