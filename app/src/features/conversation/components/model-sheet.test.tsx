// The model sheet as a view of its model (S-ModelSheet): the rows, the quiet line and the
// confirm, drawn from a hand-made SheetView.

import { describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import type { SheetRow, SheetView } from "../model/model-sheet";
import { labelLeft } from "./effort-slider";
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
    favourite: false,
    starrable: true,
    starLabel: "Make Opus 5.5 the favourite of work",
    ...overrides,
  };
}

const FILTER: SheetView["filter"] = {
  kinds: true,
  kind: "all",
  legacy: false,
  on: false,
  label: "Filter models, all providers, legacy models hidden",
  placeholder: "Search",
};

function view(rows: readonly SheetRow[], extra: Partial<SheetView> = {}): SheetView {
  return {
    sections: [
      { key: "work", title: "Claude API · work", note: undefined, dim: false, rows, legacy: [] },
    ],
    noMatch: undefined,
    filter: FILTER,
    effort: undefined,
    quiet: undefined,
    confirm: undefined,
    ...extra,
  };
}

async function draw(
  sheet: SheetView,
  onUseDefault = jest.fn<() => void>(),
  onStar = jest.fn<(row: SheetRow) => void>(),
  onRow = jest.fn<(row: SheetRow) => void>(),
) {
  await render(
    <SafeAreaProvider initialMetrics={METRICS}>
      <ModelSheetView
        view={sheet}
        empty={undefined}
        machineName="desktop"
        quiet={false}
        onStar={onStar}
        onRow={onRow}
        onEffort={jest.fn()}
        onUseDefault={onUseDefault}
        onQuiet={jest.fn()}
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

  it("gives the selected name, the step in use and Use default the weight of the board", async () => {
    const onUseDefault = jest.fn<() => void>();
    await draw(
      view([row({ selected: true })], {
        effort: { steps: ["low", "medium", "high"], index: 2, defaultIndex: 1 },
        quiet: { text: "chosen here", useDefault: true },
      }),
      onUseDefault,
    );
    expect(screen.getByText("Opus 5.5").props.className).toContain("text-row-strong");
    expect(screen.getByText("high").props.className).toContain("text-caption-strong");
    expect(screen.getByText("medium").props.className).not.toContain("text-caption-strong");
    // "Use default" is a control of its own with a 44 pt touch, not a run of the text.
    const use = screen.getByRole("button", { name: "Use default" });
    expect(use.props.hitSlop).toMatchObject({ top: 14, bottom: 14 });
    expect(screen.getByText("Use default").props.className).toContain("text-caption-strong");
    await fireEvent.press(use);
    expect(onUseDefault).toHaveBeenCalledTimes(1);
  });
});

describe("the model sheet of round 7", () => {
  it("draws no scroll indicator and no search field of its own (the native one is the screen's)", async () => {
    await draw(view([row()]));
    expect(screen.queryByLabelText("Search models")).toBeNull();
    expect(screen.getByTestId("model-list").props.showsVerticalScrollIndicator).toBe(false);
  });

  it("sets the favourite with a tap on the star, and never selects the row", async () => {
    const onStar = jest.fn<(row: SheetRow) => void>();
    const onRow = jest.fn<(row: SheetRow) => void>();
    const one = row();
    await draw(view([one]), undefined, onStar, onRow);
    await fireEvent.press(
      screen.getByTestId(`model-star-${one.key}`, { includeHiddenElements: true }),
    );
    expect(onStar).toHaveBeenCalledWith(one);
    expect(onRow).not.toHaveBeenCalled();
  });

  it("gives VoiceOver the star as an action of the row", async () => {
    const onStar = jest.fn<(row: SheetRow) => void>();
    const one = row({ favourite: true, starLabel: "Remove the favourite of work" });
    await draw(view([one]), undefined, onStar);
    const button = screen.getByTestId(`model-row-${one.key}`);
    expect(button.props.accessibilityActions).toEqual([
      { name: "star", label: "Remove the favourite of work" },
    ]);
    await fireEvent(button, "accessibilityAction", { nativeEvent: { actionName: "star" } });
    expect(onStar).toHaveBeenCalledWith(one);
  });

  it("has no star control on a dim row, but keeps the star of a dim favourite", async () => {
    const dim = row({ disabled: true, starrable: false, favourite: true });
    await draw(view([dim]));
    expect(
      screen.queryByTestId(`model-star-${dim.key}`, { includeHiddenElements: true }),
    ).toBeNull();
    expect(screen.getByTestId(`model-row-${dim.key}`).props.accessibilityActions).toEqual([]);
  });

  it("puts the legacy rows under Legacy at the end of the account", async () => {
    const old = row({ key: "w/claude-opus-5", model: "claude-opus-5", name: "Opus 5" });
    const sheet = view([row()]);
    const section = sheet.sections[0];
    if (section === undefined) {
      throw new Error("no section");
    }
    await draw({ ...sheet, sections: [{ ...section, legacy: [old] }] });
    expect(screen.getByTestId("model-legacy-label")).toHaveTextContent("Legacy");
    expect(screen.getByText("Opus 5")).toBeOnTheScreen();
  });

  it("says when a search finds nothing", async () => {
    await draw({ ...view([]), sections: [], noMatch: 'no model matches "xyz"' });
    expect(screen.getByTestId("model-no-match")).toHaveTextContent('no model matches "xyz"');
  });
});

describe("the effort slider", () => {
  it("centres each label under its stop, from the thumb at one end to the other", () => {
    // A 300 pt track: the thumb centre runs from 14 to 286.
    expect(labelLeft(0, 3, 300)).toBe(14 - 28);
    expect(labelLeft(1, 3, 300)).toBe(150 - 28);
    expect(labelLeft(2, 3, 300)).toBe(286 - 28);
    expect(labelLeft(4, 5, 300)).toBe(286 - 28);
  });

  it("draws one label per effort of the model, and the change of a step adds no row", async () => {
    const three = view([row({ selected: true })], {
      effort: { steps: ["low", "medium", "high"], index: 1, defaultIndex: 1 },
      quiet: { text: "Opus 5.5 · 200k context", useDefault: false },
    });
    await draw(three);
    expect(screen.queryByRole("button", { name: /^xhigh effort/ })).toBeNull();
    expect(screen.getAllByRole("button", { name: / effort/ })).toHaveLength(3);
    // A changed effort: the confirm is the native "Use" of the header, so the panel adds
    // nothing under the slider.
    const changed = {
      ...three,
      confirm: { label: "Use Opus 5.5 · high", next: false, compacts: false, pending: false },
    };
    await draw(changed);
    expect(screen.queryByTestId("model-cost-line")).toBeNull();
    expect(screen.getByTestId("model-quiet-line")).toHaveTextContent("Opus 5.5 · 200k context");
  });
});
