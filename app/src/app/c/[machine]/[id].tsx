import { useLocalSearchParams } from "expo-router";

import { ConversationScreen } from "@/features/conversation/screens/conversation-screen";

export { ErrorBoundary } from "expo-router";

// NOTE: The params are not parsed yet; the zod schema of the route params comes later.
export default function Conversation() {
  const { machine, id } = useLocalSearchParams<{ machine: string; id: string }>();
  return <ConversationScreen machine={machine} id={id} />;
}
