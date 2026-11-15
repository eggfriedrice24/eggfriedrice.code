// The model sheet as a view of its model (S-ModelSheet): the rows, the quiet line and the
// confirm, drawn from a hand-made SheetView.

import { describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import type { SheetRow, SheetView } from "../model/model-sheet";
import { ModelSheetView } from "./model-sheet";

const METRICS = {
  frame: { x: 0, y: 0, width: 402, height: 874 },
  insets: { top: 62, left: 0, right: 0, bottom: 34 },
};

function row(overrides: Partial<SheetRow> = {}): SheetRow {
  return {
    key: "anthropic-api.work/claude-opus-5-5",
    model: "claude-opus-5-5",
    provider: "anthropic-api.work",
    name: "Opus 5.5",
    logo: "claude",
    checked: false,
    selected: false,
    disabled: false,
    gone: undefined,
    label: "Opus 5.5",
    ...overrides,
  };
}

function view(rows: readonly SheetRow[], extra: Partial<SheetView> = {}): SheetView {
  return {
    sections: [{ key: "work", title: "Claude API · work", note: undefined, dim: false, rows }],
    effort: undefined,
    quiet: undefined,
    confirm: undefined,
    ...extra,
  };
}

async function draw(sheet: SheetView, onUseDefault = jest.fn<() => void>()) {
  await render(
    <SafeAreaProvider initialMetrics={METRICS}>
      <ModelSheetView
        view={sheet}
        empty={undefined}
        machineName="desktop"
        quiet={false}
        onRow={jest.fn()}
        onEffort={jest.fn()}
        onUseDefault={onUseDefault}
        onQuiet={jest.fn()}
        onConfirm={jest.fn()}
        fallbackSlider
      />
    </SafeAreaProvider>,
  );
}

describe("the model sheet view", () => {
  it("strikes the name of a gone model through, with its words at the right end", async () => {
    await draw(
      view([
        row({
          key: "anthropic-api.work/claude-sonnet-4-5",
          name: "Sonnet 4.5",
          disabled: true,
          gone: "gone from desktop",
        }),
      ]),
    );
    const name = screen.getByText("Sonnet 4.5");
    expect(name.props.className).toContain("line-through");
    expect(name.props.className).toContain("text-text-subtle");
    // The words are a text of their own, so a long name never cuts them.
    const gone = screen.getByTestId("model-row-gone");
    expect(gone).toHaveTextContent("gone from desktop");
    expect(gone.props.className).toContain("flex-none");
    expect(name).not.toContainElement(gone);
  });

  it("keeps the name of a model that is there plain", async () => {
    await draw(view([row()]));
    expect(screen.getByText("Opus 5.5").props.className).not.toContain("line-through");
    expect(screen.queryByTestId("model-row-gone")).toBeNull();
  });
});
