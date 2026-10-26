import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { act, fireEvent, render, screen } from "@testing-library/react-native";

import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";

import { COPIED_MS, CopyButton } from "./copy-button";
import { KeyValueBox } from "./key-value-box";

jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

beforeEach(() => {
  jest.useFakeTimers();
  jest.mocked(copyText).mockClear();
  jest.mocked(haptic).mockClear();
});

afterEach(() => {
  jest.useRealTimers();
});

describe("the copy button", () => {
  it("copies, plays the light haptic and says it copied", async () => {
    await render(<CopyButton variant="code" text="fn main() {}" label="Copy code" />);
    await fireEvent.press(screen.getByRole("button", { name: "Copy code" }));
    expect(copyText).toHaveBeenCalledWith("fn main() {}");
    expect(haptic).toHaveBeenCalledWith("light");
    expect(screen.getByRole("button", { name: "Copied" })).toBeOnTheScreen();
    expect(screen.getByText("✓ copied")).toBeOnTheScreen();
  });

  it("goes back after 1.5 s (design item 8.8)", async () => {
    expect(COPIED_MS).toBe(1500);
    await render(<CopyButton variant="code" text="x" label="Copy code" />);
    await fireEvent.press(screen.getByRole("button", { name: "Copy code" }));
    await act(async () => {
      jest.advanceTimersByTime(COPIED_MS - 1);
    });
    expect(screen.getByText("✓ copied")).toBeOnTheScreen();
    await act(async () => {
      jest.advanceTimersByTime(1);
    });
    expect(screen.queryByText("✓ copied")).toBeNull();
    expect(screen.getByRole("button", { name: "Copy code" })).toBeOnTheScreen();
  });

  it("starts the 1.5 s again on a second tap", async () => {
    await render(<CopyButton variant="code" text="x" label="Copy code" />);
    await fireEvent.press(screen.getByRole("button", { name: "Copy code" }));
    await act(async () => {
      jest.advanceTimersByTime(1000);
    });
    await fireEvent.press(screen.getByRole("button", { name: "Copied" }));
    await act(async () => {
      jest.advanceTimersByTime(1000);
    });
    expect(screen.getByText("✓ copied")).toBeOnTheScreen();
    await act(async () => {
      jest.advanceTimersByTime(500);
    });
    expect(screen.queryByText("✓ copied")).toBeNull();
  });

  it("reads a function text only on the tap", async () => {
    const text = jest.fn(() => "late text");
    await render(<CopyButton variant="circle" text={text} label="Copy command" />);
    expect(text).not.toHaveBeenCalled();
    await fireEvent.press(screen.getByRole("button", { name: "Copy command" }));
    expect(copyText).toHaveBeenCalledWith("late text");
  });

  it("shows only the check in the circle variant", async () => {
    await render(<CopyButton variant="circle" text="efr device add" label="Copy command" />);
    await fireEvent.press(screen.getByRole("button", { name: "Copy command" }));
    expect(screen.getByRole("button", { name: "Copied" })).toBeOnTheScreen();
    expect(screen.queryByText("✓ copied")).toBeNull();
  });

  it("copies nothing while it is disabled (a fence that still streams)", async () => {
    await render(<CopyButton variant="code" text="x" label="Copy code" disabled />);
    const button = screen.getByRole("button", { name: "Copy code" });
    expect(button).toBeDisabled();
    await fireEvent.press(button);
    expect(copyText).not.toHaveBeenCalled();
    expect(haptic).not.toHaveBeenCalled();
  });
});

describe("the key and value box", () => {
  const rows = [
    { key: "device", value: "iPhone 17 Pro" },
    { key: "key", value: "7F3A 91C2 4D0E B6A1", fingerprint: true },
  ];

  it("labels each row with its key and value", async () => {
    await render(<KeyValueBox rows={rows} />);
    expect(screen.getByLabelText("device, iPhone 17 Pro")).toBeOnTheScreen();
    expect(screen.getByLabelText("key, 7F3A 91C2 4D0E B6A1")).toBeOnTheScreen();
  });

  it("puts one hairline between two rows", async () => {
    await render(<KeyValueBox rows={rows} />);
    expect(screen.getAllByTestId("hairline", { includeHiddenElements: true })).toHaveLength(1);
  });

  it("copies a value on a long press", async () => {
    await render(<KeyValueBox rows={rows} />);
    await fireEvent(screen.getByLabelText("key, 7F3A 91C2 4D0E B6A1"), "longPress");
    expect(copyText).toHaveBeenCalledWith("7F3A 91C2 4D0E B6A1");
    expect(haptic).toHaveBeenCalledWith("light");
  });

  it("spaces the letters of the fingerprint only", async () => {
    await render(<KeyValueBox rows={rows} />);
    expect(screen.getByText("7F3A 91C2 4D0E B6A1")).toHaveStyle({ letterSpacing: 0.5 });
    expect(screen.getByText("iPhone 17 Pro")).not.toHaveStyle({ letterSpacing: 0.5 });
  });
});
