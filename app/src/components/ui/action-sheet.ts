// A native action sheet (UIAlertController .actionSheet), such as the Forget sheet of
// Settings (design 3.16), as plain data. native/show-action-sheet.ts shows it with
// ActionSheetIOS; the functions here build the options of ActionSheetIOS and read its
// answer, so a test can check both without iOS.
//
// The cancel action is last, and iOS draws it in its own card. A tap outside the sheet
// cancels too.

/** One action of a sheet. */
export interface SheetAction {
  readonly id: string;
  readonly label: string;
  /** iOS draws a destructive action in system red. */
  readonly destructive?: boolean;
}

/** A sheet: a title, a message, the actions, and the label of cancel. */
export interface ActionSheet {
  readonly title: string;
  readonly message?: string;
  readonly actions: readonly SheetAction[];
  /** The default is "Cancel". */
  readonly cancelLabel?: string;
}

/** The options of ActionSheetIOS.showActionSheetWithOptions for `sheet`. */
export interface SheetOptions {
  readonly title: string;
  readonly message?: string;
  readonly options: string[];
  readonly cancelButtonIndex: number;
  readonly destructiveButtonIndex: number[];
  readonly userInterfaceStyle: "dark";
}

/** The options of ActionSheetIOS for `sheet`. */
export function sheetOptions(sheet: ActionSheet): SheetOptions {
  const options = sheet.actions.map((action) => action.label);
  const destructiveButtonIndex: number[] = [];
  sheet.actions.forEach((action, index) => {
    if (action.destructive === true) {
      destructiveButtonIndex.push(index);
    }
  });
  options.push(sheet.cancelLabel ?? "Cancel");
  const built: SheetOptions = {
    title: sheet.title,
    options,
    cancelButtonIndex: options.length - 1,
    destructiveButtonIndex,
    userInterfaceStyle: "dark",
  };
  if (sheet.message === undefined) {
    return built;
  }
  return { ...built, message: sheet.message };
}

/** The id of the action at `index` of the options, or undefined for cancel. */
export function sheetAnswer(sheet: ActionSheet, index: number): string | undefined {
  return sheet.actions[index]?.id;
}
