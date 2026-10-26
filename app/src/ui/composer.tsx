// The composer: a text field and a send button that sends a prompt with
// `prompt.send`. A prompt sent while a turn runs waits in efrd's queue behind it. The
// phone sends no attachments and answers no input of a running command in this version.
// The composer stays above the keyboard (KeyboardStickyView).

import { useCallback, useMemo, useState } from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, TextInput, View } from "react-native";
import { KeyboardStickyView } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { semantic } from "@/theme/palette";

import { useSession } from "./session_context";
import { BUTTON, BUTTON_OFF, common, GUTTER } from "./styles";

/** The longest prompt that the field takes, in UTF-16 units; efrd caps a frame at 1 MiB. */
const MAX_PROMPT = 100_000;

const SEND_ON = { disabled: false, busy: false } as const;
const SEND_OFF = { disabled: true, busy: false } as const;
const SEND_BUSY = { disabled: true, busy: true } as const;

/** The composer of conversation `conversationId`. */
export function Composer(props: { readonly conversationId: string; readonly connected: boolean }) {
  const { conversationId, connected } = props;
  const session = useSession();
  const insets = useSafeAreaInsets();
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const canSend = connected && !sending && text.trim().length > 0;

  const send = useCallback(async () => {
    const prompt = text;
    setSending(true);
    setError(undefined);
    let message: string | undefined;
    try {
      const result = await session.sendPrompt(conversationId, prompt);
      message = result.ok ? undefined : result.message;
    } catch {
      message = "The prompt did not reach your computer.";
    }
    setSending(false);
    setError(message);
    if (message === undefined) {
      setText((current) => (current === prompt ? "" : current));
    }
  }, [text, session, conversationId]);
  const press = useCallback(() => {
    void send();
  }, [send]);

  const offset = useMemo(() => ({ closed: 0, opened: insets.bottom }), [insets.bottom]);
  const bar = useMemo(
    () => ({ ...styles.bar, paddingBottom: Math.max(insets.bottom, 8) }),
    [insets.bottom],
  );
  return (
    <KeyboardStickyView offset={offset}>
      <View style={bar}>
        {error === undefined ? null : (
          <Text style={styles.error} accessibilityRole="alert">
            {error}
          </Text>
        )}
        <View style={styles.row}>
          <TextInput
            style={styles.input}
            value={text}
            onChangeText={setText}
            placeholder={connected ? "Ask efr" : "Not connected"}
            placeholderTextColor={semantic.text_subtle}
            multiline
            maxLength={MAX_PROMPT}
            editable={!sending}
            selectionColor={semantic.accent_primary}
            keyboardAppearance="dark"
            accessibilityLabel="Prompt"
          />
          <Pressable
            onPress={press}
            disabled={!canSend}
            accessibilityRole="button"
            accessibilityLabel="Send"
            accessibilityState={sending ? SEND_BUSY : canSend ? SEND_ON : SEND_OFF}
            style={canSend ? SEND : SEND_DIMMED}
          >
            {sending ? (
              <ActivityIndicator size="small" color={semantic.text_on_accent} />
            ) : (
              <Text style={common.buttonText}>Send</Text>
            )}
          </Pressable>
        </View>
      </View>
    </KeyboardStickyView>
  );
}

const styles = StyleSheet.create({
  bar: {
    paddingHorizontal: GUTTER,
    paddingTop: 8,
    gap: 6,
    backgroundColor: semantic.surface_1,
    borderTopColor: semantic.border_default,
    borderTopWidth: StyleSheet.hairlineWidth,
  },
  row: { flexDirection: "row", alignItems: "flex-end", gap: 8 },
  input: {
    flex: 1,
    minHeight: 44,
    maxHeight: 160,
    paddingHorizontal: 12,
    paddingTop: 11,
    paddingBottom: 11,
    borderRadius: 10,
    backgroundColor: semantic.surface_2,
    color: semantic.text_primary,
    fontSize: 16,
  },
  error: { color: semantic.state_error, fontSize: 13 },
});

const SEND = BUTTON;
const SEND_DIMMED = BUTTON_OFF;
