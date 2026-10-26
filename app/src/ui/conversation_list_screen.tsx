// The list of conversations, newest first: the title, the project folder, what the
// conversation does now, and when it last changed. A conversation that waits for an
// approval says so in the warning colour, because the phone gets no push in this
// version.

import { FlashList, type ListRenderItemInfo } from "@shopify/flash-list";
import { type Href, useRouter } from "expo-router";
import { memo, useCallback, useMemo } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { semantic } from "@/theme/palette";

import { ago, baseName } from "./format";
import { useStore, useWallClock } from "./hooks";
import type { ConversationItem } from "./session";
import { useSession } from "./session_context";
import { ConnectionBanner, StateScreen, useConnection } from "./states";
import { common, GUTTER, pressable } from "./styles";

const MINUTE = 60_000;

const keyOf = (item: ConversationItem) => item.id;

const nextMinute = (at: number) => at + MINUTE - (at % MINUTE);

/** The conversations, newest first. */
export function ConversationListScreen() {
  const session = useSession();
  const { view, notice } = useConnection();
  const items = useStore(session.conversations);
  const sorted = useMemo(() => [...items].sort((a, b) => b.updatedAt - a.updatedAt), [items]);
  const now = useWallClock(nextMinute);
  const renderItem = useCallback(
    ({ item }: ListRenderItemInfo<ConversationItem>) => <ConversationRow item={item} now={now} />,
    [now],
  );
  if (view.kind === "repair_needed" && notice !== undefined) {
    return <StateScreen notice={notice} repair />;
  }
  if (sorted.length === 0 && notice !== undefined && view.kind !== "connecting") {
    return <StateScreen notice={notice} />;
  }
  return (
    <View style={common.screen}>
      <ConnectionBanner notice={notice} />
      {sorted.length === 0 ? (
        <View style={common.center}>
          <Text style={common.bodyCenter}>
            No conversations yet. Start one in your terminal with a comma, such as{" "}
            <Text style={common.mono}>, what uses the disk</Text>.
          </Text>
        </View>
      ) : (
        <FlashList
          data={sorted}
          renderItem={renderItem}
          keyExtractor={keyOf}
          extraData={now}
          contentContainerStyle={styles.list}
        />
      )}
    </View>
  );
}

const ConversationRow = memo(function ConversationRow(props: {
  readonly item: ConversationItem;
  readonly now: number;
}) {
  const router = useRouter();
  const { item } = props;
  const status = statusOf(item.status);
  const folder = item.cwd === undefined ? undefined : baseName(item.cwd);
  const when = ago(item.updatedAt, props.now);
  const open = useCallback(() => {
    const href: Href = { pathname: "/conversation/[id]", params: { id: item.id } };
    router.push(href);
  }, [router, item.id]);
  const label = [item.title ?? "Untitled", status?.text, folder, when]
    .filter((part) => part !== undefined)
    .join(", ");
  return (
    <Pressable onPress={open} accessibilityRole="button" accessibilityLabel={label} style={ROW}>
      <View style={styles.top}>
        <Text style={styles.title} numberOfLines={1}>
          {item.title ?? "Untitled"}
        </Text>
        <Text style={common.subtle}>{when}</Text>
      </View>
      <View style={styles.top}>
        <Text style={common.muted} numberOfLines={1}>
          {folder ?? " "}
        </Text>
        {status === undefined ? null : <Text style={STATUS[status.tone]}>{status.text}</Text>}
      </View>
    </Pressable>
  );
});

function statusOf(status: string): { text: string; tone: keyof typeof STATUS } | undefined {
  switch (status) {
    case "running":
      return { text: "running", tone: "running" };
    case "awaiting_approval":
      return { text: "needs your answer", tone: "waiting" };
    default:
      return undefined;
  }
}

const styles = StyleSheet.create({
  list: { paddingVertical: 8 },
  top: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 12 },
  title: { flex: 1, color: semantic.text_bright, fontSize: 16, fontWeight: "500" },
});

const ROW = pressable({
  paddingHorizontal: GUTTER,
  paddingVertical: 12,
  gap: 4,
  borderBottomColor: semantic.border_default,
  borderBottomWidth: StyleSheet.hairlineWidth,
});

const STATUS = StyleSheet.create({
  running: { fontSize: 13, fontWeight: "600", color: semantic.state_info },
  waiting: { fontSize: 13, fontWeight: "600", color: semantic.state_warning },
});
