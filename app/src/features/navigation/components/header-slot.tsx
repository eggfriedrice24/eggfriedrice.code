// The slot under the bar of a conversation (plan section 2.5): the approval strip or the
// offline line, pinned under the transparent bar.
//
// - One approval elsewhere: a tap opens that conversation at its approval, by a push or a
//   pop back to it.
// - Two or more: a tap opens the sheet with the waiting conversations on top.

import { useRouter } from "expo-router";
import { useHeaderHeight } from "expo-router/react-navigation";

import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { conversationsHref } from "../model/routes";
import { useHeaderSlot } from "../use-header-slot";
import { useOpenConversation } from "../use-open-conversation";
import { HeaderStrip } from "./header-strip";

export interface HeaderSlotProps {
  /** The conversation on screen. */
  readonly current: ConversationRef;
}

/** The slot under the bar of the conversation `current`. */
export function HeaderSlot(props: HeaderSlotProps) {
  const content = useHeaderSlot(props.current);
  const open = useOpenConversation();
  const router = useRouter();
  const top = useHeaderHeight();
  const onPress = (): void => {
    if (content.kind === "approval") {
      open(content.conversation, { focus: "approval" });
    } else if (content.kind === "approvals") {
      router.push(conversationsHref("waiting"));
    }
  };
  return <HeaderStrip content={content} top={top} onPress={onPress} />;
}
