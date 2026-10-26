import { afterEach, beforeEach, describe, expect, it, jest } from "@jest/globals";
import { act, render, screen } from "@testing-library/react-native";

import {
  draftItem,
  eventItem,
  messageCompleted,
  promptQueued,
  run,
  textPart,
  turnCompleted,
  turnStarted,
} from "@/lib/stream/testing";

import { MessageRow } from "./message-row";
import { InScope, testScope } from "./transcript-testing";

jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const HIDDEN = { includeHiddenElements: true } as const;
const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";

const REPLY = [
  "Guard it with the turn state:",
  "",
  "```rust",
  "pub fn steer(&self) {}",
  "```",
  "",
  "Then add a test.",
].join("\n");

function started() {
  return run([eventItem(1, promptQueued(TURN, "Fix it.")), eventItem(2, turnStarted(TURN))]);
}

/** A node of the rendered tree, as `screen.toJSON()` gives it. */
interface JsonNode {
  readonly type: string;
  readonly props: Record<string, unknown>;
  readonly children: readonly (JsonNode | string)[] | null;
}

function isNode(value: unknown): value is JsonNode {
  return typeof value === "object" && value !== null && "type" in value && "props" in value;
}

// Enriched's GitHub flavour is the native EnrichedMarkdown view; CommonMark is
// EnrichedMarkdownText.
const FLAVOR_OF_VIEW: Readonly<Record<string, string>> = {
  EnrichedMarkdown: "github",
  EnrichedMarkdownText: "commonmark",
};

/** The Markdown of each enriched text, in order. */
function markdowns(): { markdown: string; flavor: string }[] {
  const found: { markdown: string; flavor: string }[] = [];
  const walk = (value: unknown): void => {
    if (Array.isArray(value)) {
      value.forEach(walk);
      return;
    }
    if (!isNode(value)) {
      return;
    }
    const flavor = FLAVOR_OF_VIEW[value.type];
    if (flavor !== undefined) {
      found.push({ markdown: String(value.props["markdown"]), flavor });
    }
    value.children?.forEach(walk);
  };
  walk(screen.toJSON());
  return found;
}

async function frames(ms: number): Promise<void> {
  await act(async () => {
    jest.advanceTimersByTime(ms);
  });
}

beforeEach(() => {
  jest.useFakeTimers();
});

afterEach(() => {
  jest.useRealTimers();
});

describe("a final message", () => {
  it("shows its Markdown and puts the fence in our code block frame", async () => {
    const { scope } = testScope(run([eventItem(3, messageCompleted(TURN, 0, REPLY))], started()));
    await render(
      <InScope scope={scope}>
        <MessageRow turnId={TURN} index={0} />
      </InScope>,
    );
    expect(markdowns()).toEqual([
      { markdown: "Guard it with the turn state:", flavor: "github" },
      { markdown: "```rust\npub fn steer(&self) {}\n```", flavor: "commonmark" },
      { markdown: "Then add a test.", flavor: "github" },
    ]);
    expect(screen.getByText("rust")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Copy code" })).toBeEnabled();
    expect(screen.queryByTestId("block-cursor", HIDDEN)).toBeNull();
  });
});

describe("a diff in a message", () => {
  it("shows a closed diff fence as diff lines in the code block frame", async () => {
    const text = "Like this:\n\n```diff\n@@ -1 +1 @@\n-old\n+new\n```\n";
    const { scope } = testScope(run([eventItem(3, messageCompleted(TURN, 0, text))], started()));
    await render(
      <InScope scope={scope}>
        <MessageRow turnId={TURN} index={0} />
      </InScope>,
    );
    expect(screen.getByText("diff")).toBeOnTheScreen();
    expect(screen.getByLabelText("removed: old")).toBeOnTheScreen();
    expect(screen.getByLabelText("added: new")).toBeOnTheScreen();
    expect(markdowns()).toEqual([{ markdown: "Like this:", flavor: "github" }]);
  });
});

describe("a streaming message", () => {
  it("shows an open fence as a streaming block, with copy off and the code cursor", async () => {
    const state = run(
      [draftItem(TURN, 2, textPart(0, 0, "Guard it:\n\n```rust\npub fn steer("))],
      started(),
    );
    const { scope } = testScope(state);
    await render(
      <InScope scope={scope}>
        <MessageRow turnId={TURN} index={0} />
      </InScope>,
    );
    await frames(1_000);
    expect(screen.getByText("rust · streaming")).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Copy code" })).toBeDisabled();
    expect(screen.getAllByTestId("block-cursor", HIDDEN)).toHaveLength(1);
    // No raw backticks in the Markdown of the message: the fence is in the frame.
    expect(markdowns()).toEqual([
      { markdown: "Guard it:", flavor: "github" },
      { markdown: "```rust\npub fn steer(", flavor: "commonmark" },
    ]);
  });

  it("reveals new text over frames, then settles when the message completes", async () => {
    const { scope, store } = testScope(started());
    await render(
      <InScope scope={scope}>
        <MessageRow turnId={TURN} index={0} />
      </InScope>,
    );
    expect(screen.queryByTestId("message-row")).toBeNull();
    await act(async () => {
      store.setState((state) =>
        run([draftItem(TURN, 2, textPart(0, 0, "Found it. The steer lands late."))], state),
      );
    });
    await frames(1_000);
    expect(markdowns()).toEqual([
      { markdown: "Found it. The steer lands late.", flavor: "github" },
    ]);
    expect(screen.getAllByTestId("block-cursor", HIDDEN)).toHaveLength(1);
    await act(async () => {
      store.setState((state) =>
        run([eventItem(3, messageCompleted(TURN, 0, "Found it. The steer lands late."))], state),
      );
    });
    await frames(100);
    expect(screen.queryByTestId("block-cursor", HIDDEN)).toBeNull();
  });
});

describe("a recycled row", () => {
  it("shows the text of its new message when the list gives it another turn", async () => {
    const other = "019a9b1c-3d00-7a10-8b20-000000000003";
    const state = run(
      [
        eventItem(3, messageCompleted(TURN, 0, "The reply of turn A.")),
        eventItem(4, turnCompleted(TURN)),
        eventItem(5, promptQueued(other, "And now?")),
        eventItem(6, turnStarted(other)),
        eventItem(7, messageCompleted(other, 0, "The reply of turn B.")),
      ],
      started(),
    );
    const { scope } = testScope(state);
    // The list recycles: one row instance gets the props of another message.
    const view = await render(
      <InScope scope={scope}>
        <MessageRow turnId={TURN} index={0} />
      </InScope>,
    );
    expect(markdowns()).toEqual([{ markdown: "The reply of turn A.", flavor: "github" }]);
    await view.rerender(
      <InScope scope={scope}>
        <MessageRow turnId={other} index={0} />
      </InScope>,
    );
    await frames(100);
    expect(markdowns()).toEqual([{ markdown: "The reply of turn B.", flavor: "github" }]);
  });
});
