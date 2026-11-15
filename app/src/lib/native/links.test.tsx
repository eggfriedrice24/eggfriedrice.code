import { beforeEach, describe, expect, it, jest } from "@jest/globals";
import { WebBrowserPresentationStyle, openBrowserAsync } from "expo-web-browser";
import { ActionSheetIOS, Alert, Linking } from "react-native";

import { LINK_TEXT, confirmTitle, openLink } from "./links";

jest.mock("expo-web-browser", () => ({
  WebBrowserPresentationStyle: { PAGE_SHEET: "pageSheet" },
  openBrowserAsync: jest.fn(() => Promise.resolve({ type: "dismiss" })),
}));

type SheetOptions = Parameters<typeof ActionSheetIOS.showActionSheetWithOptions>[0];
type SheetCallback = Parameters<typeof ActionSheetIOS.showActionSheetWithOptions>[1];

let sheets: { options: SheetOptions; callback: SheetCallback }[];
let alert: ReturnType<typeof spyAlert>;

function spyAlert() {
  return jest.spyOn(Alert, "alert").mockImplementation(() => undefined);
}

/** The options of the last action sheet. */
function lastSheet(): SheetOptions | undefined {
  return sheets[sheets.length - 1]?.options;
}

/** Presses the button `text` of the last action sheet. */
function press(text: string): void {
  const last = sheets[sheets.length - 1];
  const index = last?.options.options.indexOf(text) ?? -1;
  if (last !== undefined && index >= 0) {
    last.callback(index);
  }
}

async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  jest.restoreAllMocks();
  sheets = [];
  jest
    .spyOn(ActionSheetIOS, "showActionSheetWithOptions")
    .mockImplementation((options, callback) => {
      sheets.push({ options, callback });
    });
  alert = spyAlert();
  jest.mocked(openBrowserAsync).mockClear();
});

describe("openLink", () => {
  it("asks first in an action sheet and names the whole address", () => {
    openLink({ kind: "confirm", url: "https://docs.rs/tokio", host: "docs.rs" });
    expect(sheets).toHaveLength(1);
    expect(lastSheet()).toEqual({
      title: "Open docs.rs?",
      message: "https://docs.rs/tokio",
      options: [LINK_TEXT.open, LINK_TEXT.cancel],
      cancelButtonIndex: 1,
      userInterfaceStyle: "dark",
    });
    expect(openBrowserAsync).not.toHaveBeenCalled();
  });

  it("never shows an alert, which would lie over the lock screen", async () => {
    jest.mocked(openBrowserAsync).mockRejectedValueOnce(new Error("no browser"));
    openLink({ kind: "refuse", url: "file:///etc/passwd", reason: "no." });
    openLink({ kind: "confirm", url: "https://docs.rs", host: "docs.rs" });
    press(LINK_TEXT.open);
    await flush();
    expect(sheets).toHaveLength(3);
    expect(alert).not.toHaveBeenCalled();
  });

  it("opens a web address in a Safari sheet after Open", () => {
    openLink({ kind: "confirm", url: "https://docs.rs/tokio", host: "docs.rs" });
    press(LINK_TEXT.open);
    expect(openBrowserAsync).toHaveBeenCalledWith("https://docs.rs/tokio", {
      presentationStyle: WebBrowserPresentationStyle.PAGE_SHEET,
    });
  });

  it("opens nothing after Cancel", () => {
    openLink({ kind: "confirm", url: "https://docs.rs/tokio", host: "docs.rs" });
    press(LINK_TEXT.cancel);
    expect(openBrowserAsync).not.toHaveBeenCalled();
  });

  it("opens a mail address in the mail app", () => {
    const openURL = jest.spyOn(Linking, "openURL").mockResolvedValue(undefined);
    openLink({ kind: "confirm", url: "mailto:a@b.c", host: "" });
    expect(lastSheet()?.title).toBe(LINK_TEXT.mailTitle);
    press(LINK_TEXT.open);
    expect(openURL).toHaveBeenCalledWith("mailto:a@b.c");
    expect(openBrowserAsync).not.toHaveBeenCalled();
  });

  it("only tells why for a refused link", () => {
    openLink({
      kind: "refuse",
      url: "file:///etc/passwd",
      reason: "efr does not open file: links.",
    });
    expect(lastSheet()).toEqual({
      title: LINK_TEXT.refusedTitle,
      message: "efr does not open file: links.\n\nfile:///etc/passwd",
      options: [LINK_TEXT.ok],
      cancelButtonIndex: 0,
      userInterfaceStyle: "dark",
    });
    press(LINK_TEXT.ok);
    expect(openBrowserAsync).not.toHaveBeenCalled();
  });

  it("tells when the link does not open", async () => {
    jest.mocked(openBrowserAsync).mockRejectedValueOnce(new Error("no browser"));
    openLink({ kind: "confirm", url: "https://docs.rs", host: "docs.rs" });
    press(LINK_TEXT.open);
    await flush();
    expect(lastSheet()?.title).toBe(LINK_TEXT.failedTitle);
    expect(lastSheet()?.message).toBe("https://docs.rs");
  });

  it("titles the question by the host", () => {
    expect(confirmTitle("github.com")).toBe("Open github.com?");
    expect(confirmTitle("")).toBe("Write an email?");
  });
});
