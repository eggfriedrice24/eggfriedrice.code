import { Redirect } from "expo-router";

import { ConversationListScreen } from "@/ui/conversation_list_screen";
import { useConnection } from "@/ui/states";

// The start screen: the conversations of a paired phone, else the pairing screen.
export default function Home() {
  const { view } = useConnection();
  if (view.kind === "unpaired") {
    return <Redirect href="/pair" />;
  }
  return <ConversationListScreen />;
}
