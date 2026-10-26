import { describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";
import { Text } from "react-native";

import { InsetGroup, InsetRow, SwitchRow } from "./inset-group";

const HIDDEN = { includeHiddenElements: true } as const;

describe("the inset group", () => {
  it("shows its header, its rows and its footer", async () => {
    await render(
      <InsetGroup header="About" footer="A tap on Allow, Deny, Send and Stop.">
        <InsetRow title="Version" value="0.1.0 (14)" valueMono />
      </InsetGroup>,
    );
    expect(screen.getByRole("header", { name: "About" })).toBeOnTheScreen();
    expect(screen.getByText("Version")).toBeOnTheScreen();
    expect(screen.getByText("0.1.0 (14)").props.className).toContain("text-mono-shell");
    expect(screen.getByText("A tap on Allow, Deny, Send and Stop.")).toBeOnTheScreen();
  });

  it("puts a hairline between each two rows, 16 pt in", async () => {
    await render(
      <InsetGroup>
        <InsetRow title="desktop" />
        <InsetRow title="laptop" />
        <InsetRow title="homelab" />
      </InsetGroup>,
    );
    const lines = screen.getAllByTestId("hairline", HIDDEN);
    expect(lines).toHaveLength(2);
    expect(lines[0]?.props.className).toContain("ml-inset");
  });

  it("takes a footer with a nested mono text", async () => {
    await render(
      <InsetGroup
        footer={
          <>
            {"Run "}
            <Text className="text-mono-sm text-text-muted">efr device add</Text>
            {" on the machine, then scan its code."}
          </>
        }
      >
        <InsetRow
          title="Pair a new machine"
          tone="accent_primary"
          symbol="plus"
          onPress={() => undefined}
        />
      </InsetGroup>,
    );
    expect(screen.getByText("efr device add")).toBeOnTheScreen();
  });
});

describe("an inset row", () => {
  it("is a button when it has an action", async () => {
    const onPress = jest.fn<() => void>();
    await render(<InsetRow title="Pair a new machine" tone="accent_primary" onPress={onPress} />);
    const row = screen.getByRole("button", { name: "Pair a new machine" });
    await fireEvent.press(row);
    expect(onPress).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Pair a new machine").props.className).toContain("text-accent-primary");
  });

  it("is not a button without an action", async () => {
    await render(<InsetRow title="Version" value="0.1.0 (14)" />);
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("holds custom content and an accessory", async () => {
    await render(
      <InsetRow accessibilityLabel="desktop, connected" accessory={<Text>more</Text>}>
        <Text>desktop</Text>
      </InsetRow>,
    );
    expect(screen.getByLabelText("desktop, connected")).toBeOnTheScreen();
    expect(screen.getByText("more")).toBeOnTheScreen();
  });
});

describe("a switch row", () => {
  it("shows the value and gives the change", async () => {
    const onValueChange = jest.fn<(value: boolean) => void>();
    await render(<SwitchRow title="Haptics" value onValueChange={onValueChange} />);
    const toggle = screen.getByRole("switch", { name: "Haptics" });
    expect(toggle).toBeChecked();
    await fireEvent(toggle, "valueChange", false);
    expect(onValueChange).toHaveBeenCalledWith(false);
  });
});
