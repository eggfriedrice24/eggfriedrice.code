// Your message (design 3.4 and the S-Messages sheet): "❯" in yolk, then the text in SF
// Pro 14/21 text_bright, with 8 pt between paragraphs. No bubble and no fill.
//
// - Steered: a tag under the text in mono 11/16 text_subtle, "steered · 14:20".
// - Queued: the text in text_muted, the ❯ in text_line_number, and the tag "queued ·
//   sends when this turn ends · withdraw", where "withdraw" is a plain button in
//   accent_ui.
// - Sending (the outbox has it, efrd has not echoed it yet): the tag "sending" with the
//   braille cell of the network. Not sent: "not sent · retry".
// - The files of a prompt stand under its text as the tile row of the composer, with no
//   xmark (board S-Attach 6). A tap on a photo of this phone opens Quick Look. A prompt
//   with files on its way shows the upload in each tile and offers withdraw until its
//   `prompt.send` goes; a failed upload says so in the tag.
// - A long press opens Copy and Edit and resend.

import { useSelector } from "@tanstack/react-store";
import { Text, View } from "react-native";

import { BrailleCell } from "@/components/ui/braille-cell";
import { cn } from "@/components/ui/cn";
import { MenuView } from "@/components/ui/menu-view";
import { MonoText } from "@/components/ui/mono-text";
import { PlainButton } from "@/components/ui/plain-button";
import { devWarn } from "@/lib/dev/dev-warn";
import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import type { AppAttach } from "@/features/navigation/model/app-services";
import { NO_ATTACHMENTS } from "../model/attachments";
import { promptTiles, type TileView } from "../model/attachments-tiles";
import { clockTime } from "../model/durations";
import { paragraphs } from "../model/paragraphs";
import type { OutboxPrompt } from "../model/screen-outbox";
import { compareShallow, selectPrompt, selectSteer } from "../model/selectors";
import { AttachmentRow } from "./attachment-row";
import { useTranscriptScope } from "./use-transcript-scope";

/** The state of your message, for its tag. */
export type YourMessageState =
  | { readonly kind: "sent" }
  | { readonly kind: "steered"; readonly at: number | undefined }
  /** A steer that efrd took, before the model saw it. */
  | { readonly kind: "steering" }
  | { readonly kind: "withdrawn" }
  | { readonly kind: "queued"; readonly onWithdraw: () => void }
  /** `onWithdraw` takes back a prompt whose files still upload. */
  | { readonly kind: "sending"; readonly onWithdraw?: () => void }
  | {
      readonly kind: "failed";
      readonly onResend: () => void;
      /** Why it failed, such as a lost upload. */
      readonly reason?: string;
      readonly onWithdraw?: () => void;
    };

export interface YourMessageViewProps {
  readonly text: string;
  readonly state: YourMessageState;
  /** Puts the text in the composer again. */
  readonly onEditAndResend: (text: string) => void;
  /** The files of the prompt. */
  readonly tiles?: readonly TileView[];
  /** Quick Look of a photo tile. */
  readonly onLook?: (tile: TileView) => void;
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
      return state.onWithdraw === undefined ? "sending" : "sending ·";
    case "failed":
      return state.reason === undefined ? "not sent ·" : `not sent · ${state.reason} ·`;
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
          {props.tiles === undefined || props.tiles.length === 0 ? null : (
            <View className="mt-8">
              <AttachmentRow tiles={props.tiles} onLook={props.onLook} />
            </View>
          )}
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
      {(state.kind === "sending" || state.kind === "failed") && state.onWithdraw !== undefined ? (
        <>
          {state.kind === "failed" ? <MonoText>·</MonoText> : null}
          <PlainButton
            label="withdraw"
            accessibilityLabel="Withdraw the message and its files"
            onPress={state.onWithdraw}
          />
        </>
      ) : null}
    </View>
  );
}

export interface PromptRowProps {
  readonly turnId: string;
}

/** Quick Look of a photo tile with its preview. */
function lookAt(attach: AppAttach | null | undefined, tile: TileView): void {
  if (attach === null || attach === undefined || tile.previewUri === undefined) {
    return;
  }
  attach.native.files.quickLook(tile.previewUri, tile.name).catch((error: unknown) => {
    devWarn(`Quick Look of ${tile.name} failed: ${String(error)}`);
  });
}

/** The prompt of turn `turnId`. */
export function PromptRow(props: PromptRowProps) {
  const scope = useTranscriptScope();
  const view = useSelector(scope.store, selectPrompt(props.turnId), { compare: compareShallow });
  const withheld = useSelector(scope.store, (state) => state.withheld);
  const commandId = view.prompt?.commandId ?? "";
  const previews = useSelector(scope.attach?.attachments.store ?? NO_ATTACHMENTS, (state) =>
    state.previews.get(commandId),
  );
  if (view.prompt === undefined) {
    return null;
  }
  const turnId = props.turnId;
  const state: YourMessageState = view.queued
    ? { kind: "queued", onWithdraw: () => scope.actions.withdraw(turnId) }
    : { kind: "sent" };
  const tiles = promptTiles(view.prompt, withheld, (index) => previews?.uris[index] ?? undefined);
  return (
    <YourMessageView
      text={view.prompt.text}
      state={state}
      onEditAndResend={scope.actions.editAndResend}
      tiles={tiles}
      onLook={(tile) => lookAt(scope.attach, tile)}
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

export interface PendingPromptRowProps {
  readonly prompt: OutboxPrompt;
}

/** A prompt on its way, from the outbox. */
export function PendingPromptRow(props: PendingPromptRowProps) {
  const scope = useTranscriptScope();
  const prompt = props.prompt;
  const attach = scope.attach;
  const withdraw =
    prompt.withdrawable && attach !== null && attach !== undefined
      ? () => attach.attachments.withdraw(prompt.key)
      : undefined;
  let state: YourMessageState;
  if (prompt.failed) {
    const failed: { -readonly [K in keyof FailedState]: FailedState[K] } = {
      kind: "failed",
      onResend: () => scope.actions.resend(prompt.key),
    };
    if (prompt.reason !== undefined) {
      failed.reason = prompt.reason;
    }
    if (withdraw !== undefined) {
      failed.onWithdraw = withdraw;
    }
    state = failed;
  } else {
    state =
      withdraw === undefined ? { kind: "sending" } : { kind: "sending", onWithdraw: withdraw };
  }
  return (
    <YourMessageView
      text={prompt.text}
      state={state}
      onEditAndResend={scope.actions.editAndResend}
      tiles={prompt.tiles}
      onLook={(tile) => lookAt(attach, tile)}
    />
  );
}

type FailedState = Extract<YourMessageState, { readonly kind: "failed" }>;
