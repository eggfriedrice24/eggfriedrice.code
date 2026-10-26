// The pairing screen: it says how to pair, runs the pairing flow of src/pairing (scan,
// connect, confirm on the computer) and shows each step and each failure in plain
// words (connection.ts). A dev build also offers the demo, which plays scripted streams
// with no computer.

import { useRouter } from "expo-router";
import { useCallback, useEffect } from "react";
import { ActivityIndicator, Pressable, Text, View } from "react-native";

import { semantic } from "@/theme/palette";

import { pairingNotice } from "./connection";
import { useStore } from "./hooks";
import { useSession } from "./session_context";
import { BUTTON, BUTTON_QUIET, common, PLAIN } from "./styles";

/** The pairing screen. `onDemo` is given only in a dev build. */
export function PairingScreen(props: { readonly onDemo?: () => void }) {
  const session = useSession();
  const router = useRouter();
  const view = useStore(session.pairing.state);
  const notice = pairingNotice(view);
  useEffect(() => {
    if (view.kind === "paired") {
      router.replace("/");
    }
  }, [view.kind, router]);
  const { action } = notice;
  const run = useCallback(() => {
    if (action === "Open conversations") {
      router.replace("/");
    } else {
      session.pairing.start();
    }
  }, [action, router, session]);
  return (
    <View style={common.center}>
      <Text style={common.title} accessibilityRole="header">
        {notice.title}
      </Text>
      <Text style={common.bodyCenter}>{notice.body}</Text>
      {notice.busy ? <ActivityIndicator color={semantic.accent_primary} /> : null}
      {action === undefined ? null : (
        <Pressable onPress={run} accessibilityRole="button" style={BUTTON}>
          <Text style={common.buttonText}>{action}</Text>
        </Pressable>
      )}
      {notice.busy ? (
        <Pressable onPress={session.pairing.cancel} accessibilityRole="button" style={BUTTON_QUIET}>
          <Text style={common.buttonQuietText}>Stop</Text>
        </Pressable>
      ) : null}
      {props.onDemo === undefined || notice.busy ? null : (
        <Pressable onPress={props.onDemo} accessibilityRole="button" hitSlop={8} style={PLAIN}>
          <Text style={common.subtle}>Open the demo (dev build)</Text>
        </Pressable>
      )}
    </View>
  );
}
