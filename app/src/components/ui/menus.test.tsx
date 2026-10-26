import { afterEach, describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";
import { ActionSheetIOS, Text, processColor } from "react-native";

import { roles } from "@/theme/roles";

import type { MenuEntry } from "./menu-actions";
import { MenuView } from "./menu-view";
import { showActionSheet } from "./native/show-action-sheet";

const MACHINES: MenuEntry[] = [
  { id: "desktop", title: "desktop", subtitle: "Connected", image: "desktopcomputer", state: "on" },
  { id: "laptop", title: "laptop", subtitle: "Connecting…", image: "laptopcomputer" },
  {
    section: "pair",
    items: [{ id: "pair", title: "Pair a new machine", image: "qrcode.viewfinder" }],
  },
];

describe("the menu view", () => {
  it("opens on a tap, in the dark style, with the converted actions", async () => {
    await render(
      <MenuView title="Machine" entries={MACHINES} onAction={() => undefined}>
        <Text>desktop</Text>
      </MenuView>,
    );
    const menu = screen.getByTestId("menu-view");
    expect(menu.props.title).toBe("Machine");
    expect(menu.props.shouldOpenOnLongPress).toBe(false);
    expect(menu.props.themeVariant).toBe("dark");
    expect(menu.props.actions).toHaveLength(3);
    expect(menu.props.actions[2]).toMatchObject({ displayInline: true });
    expect(screen.getByText("desktop")).toBeOnTheScreen();
  });

  it("gives the id of the picked item", async () => {
    const onAction = jest.fn<(id: string) => void>();
    await render(
      <MenuView entries={MACHINES} onAction={onAction}>
        <Text>desktop</Text>
      </MenuView>,
    );
    await fireEvent(screen.getByTestId("menu-view"), "pressAction", {
      nativeEvent: { event: "pair" },
    });
    expect(onAction).toHaveBeenCalledWith("pair");
  });

  it("says when it opens and closes, for the open fill", async () => {
    const onOpenChange = jest.fn<(open: boolean) => void>();
    await render(
      <MenuView entries={MACHINES} onAction={() => undefined} onOpenChange={onOpenChange}>
        <Text>desktop</Text>
      </MenuView>,
    );
    await fireEvent(screen.getByTestId("menu-view"), "openMenu");
    await fireEvent(screen.getByTestId("menu-view"), "closeMenu");
    expect(onOpenChange.mock.calls).toEqual([[true], [false]]);
  });

  it("keeps the yolk of a glyph with a tone", async () => {
    await render(
      <MenuView
        title="Recent on desktop"
        entries={[
          {
            id: "a",
            title: "Fix steer race in turn loop",
            subtitle: "Needs approval · ~/code/efr",
            image: "exclamationmark.circle.fill",
            imageTone: "accent_primary",
          },
        ]}
        onAction={() => undefined}
      >
        <Text>title</Text>
      </MenuView>,
    );
    const [action] = screen.getByTestId("menu-view").props.actions;
    expect(action.image).toBe("exclamationmark.circle.fill");
    expect(action.imageColor).toBe(processColor(roles.accent_primary));
  });
});

describe("the action sheet", () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  const sheet = {
    title: "Forget desktop?",
    message: "This iPhone deletes its pairing key for desktop.",
    actions: [{ id: "forget", label: "Forget desktop", destructive: true }],
  };

  it("shows the options of iOS and gives the picked action", async () => {
    const show = jest
      .spyOn(ActionSheetIOS, "showActionSheetWithOptions")
      .mockImplementation((_options, callback) => callback(0));
    await expect(showActionSheet(sheet)).resolves.toBe("forget");
    expect(show.mock.calls[0]?.[0]).toEqual({
      title: "Forget desktop?",
      message: "This iPhone deletes its pairing key for desktop.",
      options: ["Forget desktop", "Cancel"],
      cancelButtonIndex: 1,
      destructiveButtonIndex: [0],
      userInterfaceStyle: "dark",
    });
  });

  it("gives undefined on cancel", async () => {
    jest
      .spyOn(ActionSheetIOS, "showActionSheetWithOptions")
      .mockImplementation((_options, callback) => callback(1));
    await expect(showActionSheet(sheet)).resolves.toBeUndefined();
  });
});
