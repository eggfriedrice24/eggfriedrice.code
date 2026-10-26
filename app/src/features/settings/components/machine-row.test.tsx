import { describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";
import type { HostInstance } from "react-native";

import type { MachineAction, MachineRowText } from "../model/settings-format";
import { MachineRow } from "./machine-row";

const CONNECTED: MachineRowText = {
  name: "desktop",
  dim: false,
  state: "✓ connected",
  stateTone: "state_success",
  node: "desktop.tail3f2a.ts.net",
  version: "efrd 0.0.5 · protocol 1",
  actionsLabel: "desktop actions",
};

const OFFLINE: MachineRowText = {
  name: "homelab",
  dim: true,
  state: "offline",
  stateTone: "text_subtle",
  node: undefined,
  version: undefined,
  actionsLabel: "homelab actions",
};

describe("the machine row", () => {
  it("draws the three mono lines", async () => {
    await render(<MachineRow text={CONNECTED} onAction={() => undefined} />);
    expect(screen.getByText("desktop").props.className).toContain("text-mono-name");
    expect(screen.getByText("desktop").props.className).toContain("text-text-bright");
    expect(screen.getByText("✓ connected").props.className).toContain("text-state-success");
    expect(screen.getByText("✓ connected · desktop.tail3f2a.ts.net")).toBeOnTheScreen();
    expect(screen.getByText("efrd 0.0.5 · protocol 1").props.className).toContain("text-mono-xs");
  });

  it("draws an offline machine dim, with no node and no version line", async () => {
    await render(<MachineRow text={OFFLINE} onAction={() => undefined} />);
    expect(screen.getByText("homelab").props.className).toContain("text-text-muted");
    expect(screen.getByText("offline").props.className).toContain("text-text-subtle");
    expect(screen.queryByText(/efrd/)).toBeNull();
  });

  it("fills the ellipsis while its menu is open", async () => {
    await render(<MachineRow text={CONNECTED} onAction={() => undefined} />);
    const button = screen.getByRole("button", { name: "desktop actions" });
    expect(button.props.accessibilityState).toEqual({ expanded: false });
    await fireEvent(screen.getByTestId("machine-menu-desktop"), "openMenu");
    expect(
      screen.getByRole("button", { name: "desktop actions" }).props.accessibilityState,
    ).toEqual({ expanded: true });
    await fireEvent(screen.getByTestId("machine-menu-desktop"), "closeMenu");
    expect(
      screen.getByRole("button", { name: "desktop actions" }).props.accessibilityState,
    ).toEqual({ expanded: false });
  });

  it("gives the picked action with the view of the ellipsis", async () => {
    const onAction = jest.fn<(action: MachineAction, anchor: HostInstance | null) => void>();
    await render(<MachineRow text={CONNECTED} onAction={onAction} />);
    await fireEvent(screen.getByTestId("machine-menu-desktop"), "pressAction", {
      nativeEvent: { event: "forget" },
    });
    await fireEvent(screen.getByTestId("machine-menu-desktop"), "pressAction", {
      nativeEvent: { event: "something else" },
    });
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction.mock.calls[0]?.[0]).toBe("forget");
    expect(onAction.mock.calls[0]?.[1]).not.toBeNull();
  });
});
