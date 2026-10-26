// The title of the bar with the title menu (design sections 2 and 3.14, plan section
// 2.5): `Stack.Title asChild` with our button, because the system title menu cannot carry
// the yolk dot (design note 2). It also sets the `title` of the route, which the native
// back menu lists.
//
// - The menu lists the 8 recent conversations of the machine of the screen, the waiting
//   ones first, and "All conversations…", which opens the sheet.
// - On a conversation the dot shows while another conversation waits, on any machine.
//   On the blank root it shows while a conversation of its machine waits, because the
//   menu lists those.

import { Stack, useRouter } from "expo-router";

import type { Machine } from "@/features/machines/model/machine";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { conversationsHref } from "../model/routes";
import {
  titleLabel,
  titleMenuAction,
  titleMenuEntries,
  titleMenuTitle,
} from "../model/title-menu-entries";
import { useApprovalsElsewhere } from "../use-approvals-elsewhere";
import { useOpenConversation } from "../use-open-conversation";
import { useTitleMenuRows } from "../use-title-menu";
import { TitleButton } from "./title-button";

export interface HeaderTitleProps {
  /** The machine of the screen. */
  readonly machine: Machine;
  /** The conversation on screen; undefined on the blank root. */
  readonly current: ConversationRef | undefined;
  /** The machine name on the blank root, else the title of the conversation. */
  readonly title: string;
}

/** The title of the bar. */
export function HeaderTitle(props: HeaderTitleProps) {
  const { machine, current } = props;
  const rows = useTitleMenuRows(machine.id, current);
  const elsewhere = useApprovalsElsewhere(current);
  const open = useOpenConversation();
  const router = useRouter();
  let waiting: number;
  if (current === undefined) {
    waiting = rows.filter((row) => row.glyph === "approval").length;
  } else if (elsewhere.kind === "one") {
    waiting = 1;
  } else {
    waiting = elsewhere.kind === "many" ? elsewhere.count : 0;
  }
  const onAction = (id: string): void => {
    const action = titleMenuAction(id);
    if (action?.kind === "open") {
      open(action.conversation);
    } else if (action?.kind === "all") {
      router.push(conversationsHref());
    }
  };
  return (
    <>
      <Stack.Screen options={{ title: props.title }} />
      <Stack.Title asChild>
        <TitleButton
          title={props.title}
          mono={current === undefined}
          dot={waiting > 0}
          label={titleLabel(props.title, waiting)}
          menuTitle={titleMenuTitle(machine.name)}
          entries={titleMenuEntries(rows)}
          onAction={onAction}
        />
      </Stack.Title>
    </>
  );
}
