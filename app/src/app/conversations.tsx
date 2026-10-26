import { useLocalSearchParams } from "expo-router";

import { sheetParams } from "@/features/conversations/model/sheet-schemas";
import { ConversationsScreen } from "@/features/conversations/screens/conversations-screen";

export { ErrorBoundary } from "expo-router";

export default function Conversations() {
  const params = sheetParams(useLocalSearchParams());
  return <ConversationsScreen {...params} />;
}
