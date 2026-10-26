// Shows an action sheet (components/ui/action-sheet.ts) with ActionSheetIOS. On iOS 26
// the sheet can come out of the button that opened it (`anchor`), as the Forget sheet
// does from the ellipsis button of a Settings row.

import { ActionSheetIOS, findNodeHandle, type View } from "react-native";

import { sheetAnswer, sheetOptions, type ActionSheet } from "../action-sheet";

/**
 * Shows `sheet`. The promise gives the id of the picked action, or undefined when the
 * user cancels. `anchor` is the view that opened the sheet.
 */
export function showActionSheet(
  sheet: ActionSheet,
  anchor?: View | null,
): Promise<string | undefined> {
  const options = sheetOptions(sheet);
  const node = anchor === undefined || anchor === null ? null : findNodeHandle(anchor);
  return new Promise((resolve) => {
    const shown = node === null ? options : { ...options, anchor: node };
    ActionSheetIOS.showActionSheetWithOptions(shown, (index) => {
      resolve(sheetAnswer(sheet, index));
    });
  });
}
