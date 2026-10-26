// The conversation screen: the transcript, the composer and the connection banner. It
// follows the conversation while it is open, announces new questions and the end of
// each turn to VoiceOver, and gives a warning haptic when a question comes.

import * as Haptics from "expo-haptics";
import { Stack } from "expo-router";
import { useEffect, useMemo } from "react";
import { AccessibilityInfo, View } from "react-native";

import { announcements } from "./announce";
import { Composer } from "./composer";
import { EMPTY_DETAILS, type TranscriptDetails } from "./details";
import { combineStores } from "./external_store";
import { useFrameGated, useReduceMotion, useStore } from "./hooks";
import { createRowBuilder } from "./rows";
import type { ConversationSource } from "./session";
import { useSession } from "./session_context";
import { ConnectionBanner, useConnection } from "./states";
import { common } from "./styles";
import { TranscriptList } from "./transcript_list";
import type { RowContext } from "./transcript_rows";

/** The screen of conversation `conversationId`. */
export function ConversationScreen(props: { readonly conversationId: string }) {
  const session = useSession();
  // The stores follow the conversation while this screen listens to them.
  const source = useMemo(
    () => session.conversation(props.conversationId),
    [session, props.conversationId],
  );
  return <Transcript key={source.id} source={source} />;
}

function Transcript(props: { readonly source: ConversationSource }) {
  const { source } = props;
  const { view, notice } = useConnection();
  const connected = view.kind === "online";
  const reduceMotion = useReduceMotion();
  const gatedStream = useFrameGated(source.stream);
  const gatedDetails = useFrameGated(source.details);
  const rowsStore = useMemo(() => {
    // A builder of its own for each transcript, so the rows of two never mix.
    const build = createRowBuilder();
    return combineStores(gatedDetails, gatedStream, (details, stream) =>
      build({ details, stream }),
    );
  }, [gatedDetails, gatedStream]);
  const rows = useStore(rowsStore);
  const title = useStore(source.title);
  const context = useMemo<RowContext>(
    () => ({
      conversationId: source.id,
      stream: source.stream,
      gatedStream,
      details: gatedDetails,
      connected,
      reduceMotion,
    }),
    [source, gatedStream, gatedDetails, connected, reduceMotion],
  );
  useAnnouncements(source);
  const options = useMemo(() => ({ title: title ?? "Conversation" }), [title]);
  return (
    <View style={common.screen}>
      <Stack.Screen options={options} />
      <ConnectionBanner notice={notice} />
      <View style={common.screen}>
        <TranscriptList rows={rows} context={context} />
      </View>
      <Composer conversationId={source.id} connected={connected} />
    </View>
  );
}

/** Tells VoiceOver of each new question at once and of each turn's end in its queue. */
function useAnnouncements(source: ConversationSource): void {
  useEffect(() => {
    let before: TranscriptDetails = source.details.getSnapshot();
    // The questions that wait when the screen opens are new to the reader, too.
    for (const item of announcements(EMPTY_DETAILS, before)) {
      AccessibilityInfo.announceForAccessibilityWithOptions(item.text, {
        queue: item.queue,
        priority: item.priority,
      });
    }
    return source.details.subscribe(() => {
      const after = source.details.getSnapshot();
      for (const item of announcements(before, after)) {
        AccessibilityInfo.announceForAccessibilityWithOptions(item.text, {
          queue: item.queue,
          priority: item.priority,
        });
        if (item.priority === "high") {
          void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Warning).catch(() => {});
        }
      }
      before = after;
    });
  }, [source]);
}
