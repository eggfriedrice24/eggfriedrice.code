// The connection states on screen: a banner over a screen that still shows its last
// content, and the whole-screen states "Tailscale is not connected" and "Pair this phone
// again" (connection.ts has the words).

import { useRouter } from "expo-router";
import { useCallback } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import { semantic } from "@/theme/palette";

import { type ConnectionNotice, connectionNotice } from "./connection";
import { useStore, useWallClock } from "./hooks";
import type { ConnectionView } from "./session";
import { useSession } from "./session_context";
import { BUTTON, BUTTON_QUIET, common, GUTTER, PLAIN } from "./styles";

/** The connection of the session now, and the clock of its retry countdown. */
export function useConnection(): { view: ConnectionView; notice: ConnectionNotice | undefined } {
  const session = useSession();
  const view = useStore(session.connection);
  const retryAt = "retryAt" in view ? view.retryAt : undefined;
  const now = useWallClock(
    useCallback(
      (at: number) => (retryAt !== undefined && retryAt > at ? at + 1000 - (at % 1000) : undefined),
      [retryAt],
    ),
  );
  return { view, notice: connectionNotice(view, now) };
}

/** A small bar over a screen while the connection does not work. */
export function ConnectionBanner(props: { readonly notice: ConnectionNotice | undefined }) {
  const session = useSession();
  const { notice } = props;
  if (notice === undefined) {
    return null;
  }
  return (
    <View
      style={styles.banner}
      accessibilityRole="summary"
      accessibilityLabel={`${notice.title}. ${notice.body}`}
    >
      <View style={styles.bannerText}>
        <Text style={BANNER_TITLE[notice.tone]}>{notice.title}</Text>
        {notice.retryIn === undefined ? null : (
          <Text style={common.subtle}>Next try in {notice.retryIn} s</Text>
        )}
      </View>
      {notice.canRetry ? (
        <Pressable onPress={session.reconnect} accessibilityRole="button" hitSlop={8} style={PLAIN}>
          <Text style={styles.retryText}>Try now</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

/** A whole screen that says what is wrong and what to do. */
export function StateScreen(props: {
  readonly notice: ConnectionNotice;
  readonly repair?: boolean;
}) {
  const session = useSession();
  const router = useRouter();
  const { notice } = props;
  const pressRepair = useCallback(() => {
    const repair = async () => {
      try {
        await session.forget();
      } catch {
        // The pairing screen pairs again in any case.
      }
      router.replace("/pair");
    };
    void repair();
  }, [session, router]);
  return (
    <View style={common.center}>
      <Text style={SCREEN_TITLE[notice.tone]} accessibilityRole="header">
        {notice.title}
      </Text>
      <Text style={common.bodyCenter}>{notice.body}</Text>
      {notice.retryIn === undefined ? null : (
        <Text style={common.subtle}>Next try in {notice.retryIn} s</Text>
      )}
      {props.repair === true ? (
        <Pressable onPress={pressRepair} accessibilityRole="button" style={BUTTON}>
          <Text style={common.buttonText}>Pair again</Text>
        </Pressable>
      ) : null}
      {notice.canRetry ? (
        <Pressable onPress={session.reconnect} accessibilityRole="button" style={BUTTON_QUIET}>
          <Text style={common.buttonQuietText}>Try now</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  banner: {
    flexDirection: "row",
    alignItems: "center",
    gap: 12,
    paddingHorizontal: GUTTER,
    paddingVertical: 8,
    backgroundColor: semantic.surface_2,
    borderBottomColor: semantic.border_default,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  bannerText: { flex: 1, gap: 2 },
  retryText: {
    color: semantic.accent_primary,
    fontSize: 14,
    fontWeight: "600",
    paddingHorizontal: 10,
    paddingVertical: 6,
  },
});

const BANNER = { fontSize: 14, fontWeight: "600" } as const;

const BANNER_TITLE = StyleSheet.create({
  muted: { ...BANNER, color: semantic.text_muted },
  warning: { ...BANNER, color: semantic.state_warning },
  error: { ...BANNER, color: semantic.state_error },
});

const SCREEN_TITLE = StyleSheet.create({
  muted: { ...common.title, color: semantic.text_bright },
  warning: { ...common.title, color: semantic.state_warning },
  error: { ...common.title, color: semantic.state_error },
});
