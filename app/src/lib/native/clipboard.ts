// Copying text to the clipboard. A selection in the transcript cannot span two text
// views, so each message has its own "Copy" action (its raw Markdown), and each code
// block has the copy button of the Markdown view.
//
// NOTE: React Native 0.88 still has its own Clipboard, but marks it deprecated, and
// expo-clipboard is not in the app's list of packages (R4 of the research). This is the
// one place that copies, so a change to expo-clipboard changes only this file.

import { Clipboard } from "react-native";

/** Puts `text` on the clipboard. */
export function copyText(text: string): void {
  Clipboard.setString(text);
}
