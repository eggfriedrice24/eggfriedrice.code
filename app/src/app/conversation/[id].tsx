import { useLocalSearchParams } from "expo-router";

import { ConversationScreen } from "@/ui/conversation_screen";

// One conversation: its transcript and the composer.
export default function Conversation() {
  const { id } = useLocalSearchParams<{ id: string }>();
  return <ConversationScreen conversationId={id} />;
}
