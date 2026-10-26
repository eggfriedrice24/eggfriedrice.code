// Opening a link of the transcript. The model writes the links, so the app never opens
// one on a tap: it shows the whole address and asks first (the rules of app/README.md).
// A web address then opens in SFSafariViewController through expo-web-browser (design
// 3.5), so the user stays in the app. A mail address opens the mail app.
//
// The rule that decides is `linkAction` of features/conversation/model/links.ts. lib
// cannot import a feature, so this file names the shape of its result again
// (`LinkDecision`), and a caller passes `linkAction(url)` to `openLink`.

import { Alert, Linking } from "react-native";
import { WebBrowserPresentationStyle, openBrowserAsync } from "expo-web-browser";

/** What to do with a link: the shape of `linkAction` in the conversation model. */
export type LinkDecision =
  | { readonly kind: "confirm"; readonly url: string; readonly host: string }
  | { readonly kind: "refuse"; readonly url: string; readonly reason: string };

/** The texts of the questions. A test reads them. */
export const LINK_TEXT = {
  refusedTitle: "efr did not open the link",
  failedTitle: "The link did not open",
  mailTitle: "Write an email?",
  cancel: "Cancel",
  open: "Open",
} as const;

/** The title of the question before the app opens a link to `host`. */
export function confirmTitle(host: string): string {
  return host === "" ? LINK_TEXT.mailTitle : `Open ${host}?`;
}

/** Asks, then opens the link of `decision`. A refused link only gets an alert. */
export function openLink(decision: LinkDecision): void {
  if (decision.kind === "refuse") {
    Alert.alert(LINK_TEXT.refusedTitle, `${decision.reason}\n\n${decision.url}`);
    return;
  }
  const url = decision.url;
  const mail = decision.host === "";
  Alert.alert(
    confirmTitle(decision.host),
    url,
    [
      { text: LINK_TEXT.cancel, style: "cancel" },
      { text: LINK_TEXT.open, onPress: () => open(url, mail) },
    ],
    { cancelable: true },
  );
}

function open(url: string, mail: boolean): void {
  const opened = mail
    ? Linking.openURL(url)
    : openBrowserAsync(url, { presentationStyle: WebBrowserPresentationStyle.PAGE_SHEET });
  opened.catch(() => {
    Alert.alert(LINK_TEXT.failedTitle, url);
  });
}
