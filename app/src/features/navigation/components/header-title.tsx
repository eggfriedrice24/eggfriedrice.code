// The title of the bar (design sections 2 and 3.14, plan section 2.5): `Stack.Title
// asChild` with our button, because the system title cannot carry the yolk dot (design
// note 2). It also sets the `title` of the route, which the native back menu lists.
//
// - A tap opens the conversations sheet, the one place to switch (changed 2026-10-26:
//   it was a menu of the 8 recent conversations of the machine).
// - The dot shows while a conversation other than the one on screen waits for an
//   answer, on any machine, as the sheet lists them all.

import { Stack, useRouter } from "expo-router";

import type { Machine } from "@/features/machines/model/machine";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import { conversationsSheetHref } from "../model/routes";
import { titleLabel } from "../model/title-label";
import { useApprovalsElsewhere } from "../use-approvals-elsewhere";
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
  const elsewhere = useApprovalsElsewhere(current);
  const router = useRouter();
  let waiting = 0;
  if (elsewhere.kind === "one") {
    waiting = 1;
  } else if (elsewhere.kind === "many") {
    waiting = elsewhere.count;
  }
  return (
    <>
      <Stack.Screen options={{ title: props.title }} />
      <Stack.Title asChild>
        <TitleButton
          title={props.title}
          mono={current === undefined}
          dot={waiting > 0}
          label={titleLabel(props.title, waiting)}
          onPress={() => router.push(conversationsSheetHref(machine.id))}
        />
      </Stack.Title>
    </>
  );
}
