// Copying text to the clipboard. A selection in the transcript cannot span two text
// views, so each message has its own "Copy" action (its raw Markdown), and each code
// block has the copy button of the Markdown view.
//
// expo-clipboard does the copy, because React Native 0.88 marks its own Clipboard as
// deprecated. This is the one place that copies, so the copy button
// (src/components/ui/copy-button.tsx) and the long press actions all come through here.

import { setStringAsync } from "expo-clipboard";

/**
 * Puts `text` on the clipboard. The promise gives true when iOS took the text. It never
 * rejects: a failed copy gives false, because a caller has no other thing to do.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    return await setStringAsync(text);
  } catch {
    return false;
  }
}
