import { useLocalSearchParams } from "expo-router";

import { ConversationScreen } from "@/features/conversation/screens/conversation-screen";
import { conversationParamsSchema } from "@/features/navigation/model/route-schemas";

export { ErrorBoundary } from "expo-router";

// `?focus=approval` scrolls to the approval that waits. Bad params show a short notice.
export default function Conversation() {
  const params = conversationParamsSchema.safeParse(useLocalSearchParams());
  if (!params.success) {
    return <ConversationScreen conversation={undefined} />;
  }
  const { machine, id, focus } = params.data;
  return <ConversationScreen conversation={{ machine, id }} focus={focus} />;
}
