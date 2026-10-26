import { beforeEach, describe, expect, it, jest } from "@jest/globals";
import { WebBrowserPresentationStyle, openBrowserAsync } from "expo-web-browser";
import { Alert, Linking, type AlertButton } from "react-native";

import { LINK_TEXT, confirmTitle, openLink } from "./links";

jest.mock("expo-web-browser", () => ({
  WebBrowserPresentationStyle: { PAGE_SHEET: "pageSheet" },
  openBrowserAsync: jest.fn(() => Promise.resolve({ type: "dismiss" })),
}));

let alert: ReturnType<typeof spyAlert>;

function spyAlert() {
  return jest.spyOn(Alert, "alert").mockImplementation(() => undefined);
}

/** The buttons of the last alert. */
function lastButtons(): AlertButton[] {
  const calls = alert.mock.calls;
  return calls[calls.length - 1]?.[2] ?? [];
}

/** Presses the button `text` of the last alert. */
function press(text: string): void {
  lastButtons()
    .find((button) => button.text === text)
    ?.onPress?.();
}

async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  jest.restoreAllMocks();
  alert = spyAlert();
  jest.mocked(openBrowserAsync).mockClear();
});

describe("openLink", () => {
  it("asks first and names the whole address", () => {
    openLink({ kind: "confirm", url: "https://docs.rs/tokio", host: "docs.rs" });
    expect(alert).toHaveBeenCalledTimes(1);
    const [title, message] = alert.mock.calls[0] ?? [];
    expect(title).toBe("Open docs.rs?");
    expect(message).toBe("https://docs.rs/tokio");
    expect(lastButtons().map((button) => button.text)).toEqual([LINK_TEXT.cancel, LINK_TEXT.open]);
    expect(openBrowserAsync).not.toHaveBeenCalled();
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
    expect(alert.mock.calls[0]?.[0]).toBe(LINK_TEXT.mailTitle);
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
    expect(alert).toHaveBeenCalledWith(
      LINK_TEXT.refusedTitle,
      "efr does not open file: links.\n\nfile:///etc/passwd",
    );
    expect(lastButtons()).toEqual([]);
    expect(openBrowserAsync).not.toHaveBeenCalled();
  });

  it("tells when the link does not open", async () => {
    jest.mocked(openBrowserAsync).mockRejectedValueOnce(new Error("no browser"));
    openLink({ kind: "confirm", url: "https://docs.rs", host: "docs.rs" });
    press(LINK_TEXT.open);
    await flush();
    expect(alert).toHaveBeenLastCalledWith(LINK_TEXT.failedTitle, "https://docs.rs");
  });

  it("titles the question by the host", () => {
    expect(confirmTitle("github.com")).toBe("Open github.com?");
    expect(confirmTitle("")).toBe("Write an email?");
  });
});
