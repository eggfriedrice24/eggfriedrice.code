import { describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";
import { View } from "react-native";

import { BorderedButton } from "./bordered-button";
import { GlassButton, GlassGroup, GlassGroupItem } from "./glass-button";
import { PlainButton } from "./plain-button";
import { PrimaryButton } from "./primary-button";

describe("the primary button", () => {
  it("presses with its label", async () => {
    const onPress = jest.fn<() => void>();
    await render(<PrimaryButton label="Scan code" symbol="qrcode.viewfinder" onPress={onPress} />);
    await fireEvent.press(screen.getByRole("button", { name: "Scan code" }));
    expect(onPress).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Scan code")).toBeOnTheScreen();
  });

  it("has the pairing size by default and the approval size on request", async () => {
    await render(<PrimaryButton label="Start" onPress={() => undefined} />);
    expect(screen.getByRole("button").props.className).toContain("h-button rounded-control");
    await render(<PrimaryButton label="Allow" size="medium" grow onPress={() => undefined} />);
    const allow = screen.getByRole("button", { name: "Allow" });
    expect(allow.props.className).toContain("h-hit rounded-box");
    expect(allow.props.className).toContain("flex-1");
  });

  it("shows the braille cell in place of the label while it sends", async () => {
    const onPress = jest.fn<() => void>();
    await render(
      <PrimaryButton
        label="Allow"
        size="medium"
        pending
        pendingLabel="Allowing"
        onPress={onPress}
      />,
    );
    const button = screen.getByRole("button", { name: "Allowing" });
    expect(button).toBeDisabled();
    expect(button).toBeBusy();
    expect(screen.queryByText("Allow")).toBeNull();
    expect(screen.getByTestId("braille-cell", { includeHiddenElements: true })).toBeOnTheScreen();
    await fireEvent.press(button);
    expect(onPress).not.toHaveBeenCalled();
  });

  it("does not press when it is disabled, and is at 40%", async () => {
    const onPress = jest.fn<() => void>();
    await render(<PrimaryButton label="Start" disabled onPress={onPress} />);
    const button = screen.getByRole("button", { name: "Start" });
    expect(button).toBeDisabled();
    expect(button.props.className).toContain("opacity-40");
    await fireEvent.press(button);
    expect(onPress).not.toHaveBeenCalled();
  });
});

describe("the bordered button", () => {
  it("presses with its label", async () => {
    const onPress = jest.fn<() => void>();
    await render(<BorderedButton label="Cancel" onPress={onPress} />);
    await fireEvent.press(screen.getByRole("button", { name: "Cancel" }));
    expect(onPress).toHaveBeenCalledTimes(1);
  });

  it("is Deny at 40% and does not press while the answer is sent", async () => {
    const onPress = jest.fn<() => void>();
    await render(<BorderedButton label="Deny" size="medium" grow disabled onPress={onPress} />);
    const deny = screen.getByRole("button", { name: "Deny" });
    expect(deny).toBeDisabled();
    expect(deny.props.className).toContain("opacity-40");
    expect(deny.props.className).toContain("border-border-default");
    await fireEvent.press(deny);
    expect(onPress).not.toHaveBeenCalled();
  });
});

describe("the plain button", () => {
  it("presses with its label and reaches a 44 pt hit area", async () => {
    const onPress = jest.fn<() => void>();
    await render(<PlainButton label="retry" onPress={onPress} />);
    const button = screen.getByRole("button", { name: "retry" });
    expect(button.props.hitSlop).toEqual({ top: 10, bottom: 10 });
    await fireEvent.press(button);
    expect(onPress).toHaveBeenCalledTimes(1);
  });

  it("is 32 pt high across a block", async () => {
    await render(
      <PlainButton label="show all 14 lines" height="row" wide onPress={() => undefined} />,
    );
    const button = screen.getByRole("button", { name: "show all 14 lines" });
    expect(button.props.hitSlop).toEqual({ top: 6, bottom: 6 });
    expect(button.props.className).toContain("h-32");
  });

  it("can have a longer VoiceOver label", async () => {
    await render(
      <PlainButton
        label="withdraw"
        accessibilityLabel="Withdraw the queued message"
        onPress={() => undefined}
      />,
    );
    expect(screen.getByRole("button", { name: "Withdraw the queued message" })).toBeOnTheScreen();
  });
});

describe("the glass buttons", () => {
  it("press with their labels", async () => {
    const close = jest.fn<() => void>();
    await render(<GlassButton symbol="xmark" symbolSize={15} label="Close" onPress={close} />);
    await fireEvent.press(screen.getByRole("button", { name: "Close" }));
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("say when their menu is open", async () => {
    await render(
      <GlassButton
        symbol="line.3.horizontal.decrease"
        symbolSize={20}
        label="Machine: desktop"
        open
        onPress={() => undefined}
      />,
    );
    expect(screen.getByRole("button", { name: "Machine: desktop" })).toBeExpanded();
  });

  it("put items in one group", async () => {
    const settings = jest.fn<() => void>();
    const add = jest.fn<() => void>();
    await render(
      <View>
        <GlassGroup>
          <GlassGroupItem symbol="gearshape" symbolSize={21} label="Settings" onPress={settings} />
          <GlassGroupItem symbol="plus" symbolSize={20} label="New conversation" onPress={add} />
        </GlassGroup>
      </View>,
    );
    await fireEvent.press(screen.getByRole("button", { name: "Settings" }));
    await fireEvent.press(screen.getByRole("button", { name: "New conversation" }));
    expect(settings).toHaveBeenCalledTimes(1);
    expect(add).toHaveBeenCalledTimes(1);
  });
});
