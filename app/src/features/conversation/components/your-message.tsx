// Your message (design 3.4 and the S-Messages sheet): "❯" in yolk, then the text in SF
// Pro 14/21 text_bright, with 8 pt between paragraphs. No bubble and no fill.
//
// - Steered: a tag under the text in mono 11/16 text_subtle, "steered · 14:20".
// - Queued: the text in text_muted, the ❯ in text_line_number, and the tag "queued ·
//   sends when this turn ends · withdraw", where "withdraw" is a plain button in
//   accent_ui.
// - Sending (the outbox has it, efrd has not echoed it yet): the tag "sending" with the
//   braille cell of the network. Not sent: "not sent · retry".
// - A long press opens Copy and Edit and resend.

import { useSelector } from "@tanstack/react-store";
import { Text, View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { cn } from "@/components/ui/cn";
import { MenuView } from "@/components/ui/menu-view";
import { MonoText } from "@/components/ui/mono-text";
import { PlainButton } from "@/components/ui/plain-button";
import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import { clockTime } from "../model/durations";
import { paragraphs } from "../model/paragraphs";
import { compareShallow, selectPrompt, selectSteer } from "../model/selectors";
import { useTranscriptScope } from "./use-transcript-scope";

/** The state of your message, for its tag. */
export type YourMessageState =
  | { readonly kind: "sent" }
  | { readonly kind: "steered"; readonly at: number | undefined }
  /** A steer that efrd took, before the model saw it. */
  | { readonly kind: "steering" }
  | { readonly kind: "withdrawn" }
  | { readonly kind: "queued"; readonly onWithdraw: () => void }
  | { readonly kind: "sending" }
  | { readonly kind: "failed"; readonly onResend: () => void };

export interface YourMessageViewProps {
  readonly text: string;
  readonly state: YourMessageState;
  /** Puts the text in the composer again. */
  readonly onEditAndResend: (text: string) => void;
}

const MENU = [
  { id: "copy", title: "Copy", image: "doc.on.doc" },
  { id: "edit", title: "Edit and resend", image: "pencil" },
] as const;

/** The words of a tag, without its button. */
export function tagText(state: YourMessageState): string | undefined {
  switch (state.kind) {
    case "steered":
      return state.at === undefined ? "steered" : `steered · ${clockTime(state.at)}`;
    case "steering":
      return "steering";
    case "withdrawn":
      return "withdrawn";
    case "queued":
      return "queued · sends when this turn ends ·";
    case "sending":
      return "sending";
    case "failed":
      return "not sent ·";
    default:
      return undefined;
  }
}

/** Your message. */
export function YourMessageView(props: YourMessageViewProps) {
  const state = props.state;
  const quiet = state.kind === "queued" || state.kind === "withdrawn";
  const parts = paragraphs(props.text);
  const tag = tagText(state);
  const onAction = (id: string): void => {
    if (id === "copy") {
      void copyText(props.text);
      haptic("light");
    } else {
      props.onEditAndResend(props.text);
    }
  };
  return (
    <MenuView entries={MENU} onAction={onAction} openOnLongPress testID="your-message-menu">
      <View className="flex-row gap-10">
        <Text
          accessibilityElementsHidden
          importantForAccessibility="no"
          className={cn("text-body", quiet ? "text-text-line-number" : "text-accent-primary")}
        >
          ❯
        </Text>
        <View className="flex-1">
          <View
            accessible
            accessibilityLabel={`You: ${props.text}${tag === undefined ? "" : `, ${tag}`}`}
            className="gap-8"
          >
            {parts.map((paragraph) => (
              <Text
                key={paragraph.key}
                className={cn("text-body", quiet ? "text-text-muted" : "text-text-bright")}
              >
                {paragraph.text}
              </Text>
            ))}
          </View>
          {tag === undefined ? null : <Tag state={state} text={tag} />}
        </View>
      </View>
    </MenuView>
  );
}

function Tag(props: { readonly state: YourMessageState; readonly text: string }) {
  const state = props.state;
  return (
    <View className="mt-2 flex-row flex-wrap items-center gap-6">
      {state.kind === "sending" ? <BrailleCell tint="wait" /> : null}
      <MonoText>{props.text}</MonoText>
      {state.kind === "queued" ? (
        <PlainButton
          label="withdraw"
          accessibilityLabel="Withdraw the queued message"
          onPress={state.onWithdraw}
        />
      ) : null}
      {state.kind === "failed" ? (
        <PlainButton label="retry" accessibilityLabel="Send it again" onPress={state.onResend} />
      ) : null}
    </View>
  );
}

export interface PromptRowProps {
  readonly turnId: string;
}

/** The prompt of turn `turnId`. */
export function PromptRow(props: PromptRowProps) {
  const scope = useTranscriptScope();
  const view = useSelector(scope.store, selectPrompt(props.turnId), { compare: compareShallow });
  if (view.prompt === undefined) {
    return null;
  }
  const turnId = props.turnId;
  const state: YourMessageState = view.queued
    ? { kind: "queued", onWithdraw: () => scope.actions.withdraw(turnId) }
    : { kind: "sent" };
  return (
    <YourMessageView
      text={view.prompt.text}
      state={state}
      onEditAndResend={scope.actions.editAndResend}
    />
  );
}

export interface SteerRowProps {
  readonly turnId: string;
  readonly seq: number;
}

/** A steer of turn `turnId`. */
export function SteerRow(props: SteerRowProps) {
  const scope = useTranscriptScope();
  const steer = useSelector(scope.store, selectSteer(props.turnId, props.seq));
  if (steer === undefined) {
    return null;
  }
  const state: YourMessageState =
    steer.status === "delivered"
      ? { kind: "steered", at: steer.at }
      : steer.status === "withdrawn"
        ? { kind: "withdrawn" }
        : { kind: "steering" };
  return (
    <YourMessageView
      text={steer.text}
      state={state}
      onEditAndResend={scope.actions.editAndResend}
    />
  );
}

/** A prompt in the outbox that efrd has not echoed yet. */
export interface PendingPrompt {
  /** The outbox key of the entry (its command id). */
  readonly key: string;
  readonly text: string;
  readonly failed: boolean;
}

export interface PendingPromptRowProps {
  readonly prompt: PendingPrompt;
}

/** A prompt on its way, from the outbox. */
export function PendingPromptRow(props: PendingPromptRowProps) {
  const scope = useTranscriptScope();
  const prompt = props.prompt;
  const state: YourMessageState = prompt.failed
    ? { kind: "failed", onResend: () => scope.actions.resend(prompt.key) }
    : { kind: "sending" };
  return (
    <YourMessageView
      text={prompt.text}
      state={state}
      onEditAndResend={scope.actions.editAndResend}
    />
  );
}
