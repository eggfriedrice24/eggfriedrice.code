import { beforeEach, describe, expect, it, jest } from "@jest/globals";
import { fireEvent, render, screen } from "@testing-library/react-native";

import { copyText } from "@/lib/native/clipboard";
import { haptic } from "@/lib/native/haptics";
import type { Attachment } from "@/lib/protocol/generated";
import { eventItem, promptQueued, run, turnStarted } from "@/lib/stream/testing";

import { UPLOAD_LOST } from "../model/attachments-format";
import { testAttach } from "../model/attachments-testing";
import {
  CONVERSATION_A,
  DESKTOP,
  FILE_A,
  FILE_B,
  outboxFile,
  outboxPrompt,
} from "../model/outbox-testing";
import { outboxPrompts } from "../model/screen-outbox";
import { InScope, testScope } from "./transcript-testing";
import { PendingPromptRow, PromptRow, YourMessageView } from "./your-message";

jest.mock("@/lib/native/clipboard", () => ({
  copyText: jest.fn(() => Promise.resolve(true)),
}));
jest.mock("@/lib/native/haptics", () => ({ haptic: jest.fn() }));

const HIDDEN = { includeHiddenElements: true } as const;
const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";
const OTHER = "019a9b1c-3d00-7a10-8b20-000000000003";

beforeEach(() => {
  jest.mocked(copyText).mockClear();
  jest.mocked(haptic).mockClear();
});

describe("your message", () => {
  it("shows the yolk prompt glyph and the text in bright", async () => {
    await render(
      <YourMessageView
        text="Run the tests again."
        state={{ kind: "sent" }}
        onEditAndResend={jest.fn()}
      />,
    );
    expect(screen.getByText("❯", HIDDEN).props.className).toContain("text-accent-primary");
    expect(screen.getByText("Run the tests again.").props.className).toContain("text-text-bright");
    expect(screen.getByLabelText("You: Run the tests again.")).toBeOnTheScreen();
  });

  it("puts each paragraph in its own text", async () => {
    await render(
      <YourMessageView
        text={"Two things before you push.\n\nFirst, keep the reader."}
        state={{ kind: "sent" }}
        onEditAndResend={jest.fn()}
      />,
    );
    expect(screen.getByText("Two things before you push.")).toBeOnTheScreen();
    expect(screen.getByText("First, keep the reader.")).toBeOnTheScreen();
  });

  it("tags a steer with its time", async () => {
    const at = new Date(2026, 9, 26, 14, 20).getTime();
    await render(
      <YourMessageView
        text="Use TurnState, not a new enum."
        state={{ kind: "steered", at }}
        onEditAndResend={jest.fn()}
      />,
    );
    expect(screen.getByText("steered · 14:20")).toBeOnTheScreen();
  });

  it("dims a queued message and offers withdraw", async () => {
    const onWithdraw = jest.fn<() => void>();
    await render(
      <YourMessageView
        text="Also add a regression test."
        state={{ kind: "queued", onWithdraw }}
        onEditAndResend={jest.fn()}
      />,
    );
    expect(screen.getByText("❯", HIDDEN).props.className).toContain("text-text-line-number");
    expect(screen.getByText("Also add a regression test.").props.className).toContain(
      "text-text-muted",
    );
    expect(screen.getByText("queued · sends when this turn ends ·")).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Withdraw the queued message" }));
    expect(onWithdraw).toHaveBeenCalledTimes(1);
  });

  it("shows a prompt on its way, and one that did not go", async () => {
    const onResend = jest.fn<() => void>();
    await render(
      <YourMessageView text="Hi" state={{ kind: "sending" }} onEditAndResend={jest.fn()} />,
    );
    expect(screen.getByText("sending")).toBeOnTheScreen();
    await render(
      <YourMessageView
        text="Hi"
        state={{ kind: "failed", onResend }}
        onEditAndResend={jest.fn()}
      />,
    );
    await fireEvent.press(screen.getByRole("button", { name: "Send it again" }));
    expect(onResend).toHaveBeenCalledTimes(1);
  });

  it("opens Copy and Edit and resend on a long press", async () => {
    const onEditAndResend = jest.fn<(text: string) => void>();
    await render(
      <YourMessageView text="Fix it." state={{ kind: "sent" }} onEditAndResend={onEditAndResend} />,
    );
    const menu = screen.getByTestId("your-message-menu");
    expect(menu.props.shouldOpenOnLongPress).toBe(true);
    await fireEvent(menu, "pressAction", { nativeEvent: { event: "copy" } });
    expect(copyText).toHaveBeenCalledWith("Fix it.");
    expect(haptic).toHaveBeenCalledWith("light");
    await fireEvent(menu, "pressAction", { nativeEvent: { event: "edit" } });
    expect(onEditAndResend).toHaveBeenCalledWith("Fix it.");
  });
});

describe("the prompt row", () => {
  it("reads the prompt of its turn, and withdraws it while it waits", async () => {
    const state = run([
      eventItem(1, promptQueued(OTHER, "Find the race.")),
      eventItem(2, turnStarted(OTHER)),
      eventItem(3, promptQueued(TURN, "Also add a test.")),
    ]);
    const { scope, calls } = testScope(state);
    await render(
      <InScope scope={scope}>
        <PromptRow turnId={TURN} />
      </InScope>,
    );
    await fireEvent.press(screen.getByRole("button", { name: "Withdraw the queued message" }));
    expect(calls).toEqual([{ action: "withdraw", turnId: TURN }]);
  });

  it("shows a running prompt without a tag", async () => {
    const state = run([eventItem(1, promptQueued(TURN, "Go.")), eventItem(2, turnStarted(TURN))]);
    const { scope } = testScope(state);
    await render(
      <InScope scope={scope}>
        <PromptRow turnId={TURN} />
      </InScope>,
    );
    expect(screen.getByLabelText("You: Go.")).toBeOnTheScreen();
    expect(screen.queryByRole("button")).toBeNull();
  });
});

describe("the pending prompt row", () => {
  it("sends a failed prompt again by its key", async () => {
    const { scope, calls } = testScope(run([]));
    await render(
      <InScope scope={scope}>
        <PendingPromptRow prompt={outboxPrompt({ key: "cmd-1", text: "Hi", failed: true })} />
      </InScope>,
    );
    await fireEvent.press(screen.getByRole("button", { name: "Send it again" }));
    expect(calls).toEqual([{ action: "resend", key: "cmd-1" }]);
  });

  it("shows the upload of each file and takes the prompt back with withdraw", async () => {
    const test = testAttach();
    const entry = test.outbox.enqueue({
      machine: DESKTOP,
      target: { kind: "conversation", conversationId: CONVERSATION_A },
      text: "Compare it with the spec.",
      attachments: [
        { ...outboxFile(FILE_A, 2_400_000, "IMG_2044.jpg"), received: 960_000 },
        { ...outboxFile(FILE_B, 640_000, "spec.pdf"), kind: "pdf" },
      ],
    });
    const [prompt] = outboxPrompts(test.outbox.store.get(), DESKTOP, CONVERSATION_A);
    const { scope } = testScope(run([]));
    await render(
      <InScope scope={{ ...scope, attach: test }}>
        {prompt === undefined ? null : <PendingPromptRow prompt={prompt} />}
      </InScope>,
    );
    expect(
      screen.getByLabelText("Photo, IMG_2044.jpg, 2.4 MB, uploading 40 percent"),
    ).toBeOnTheScreen();
    expect(screen.getByLabelText("PDF, spec.pdf, 640 KB, uploading 0 percent")).toBeOnTheScreen();
    expect(screen.getByText("sending ·")).toBeOnTheScreen();
    await fireEvent.press(
      screen.getByLabelText("Photo, IMG_2044.jpg, 2.4 MB, uploading 40 percent"),
    );
    expect(test.files.looks).toEqual(["file:///caches/IMG_2044.jpg-preview IMG_2044.jpg"]);
    await fireEvent.press(
      screen.getByRole("button", { name: "Withdraw the message and its files" }),
    );
    expect(test.outbox.store.get().entries).toEqual([]);
    expect(test.discards).toEqual(["1:a", "1:b"]);
    expect(entry.attachments).toHaveLength(2);
  });

  it("says why the upload failed and offers retry and withdraw", async () => {
    const { scope } = testScope(run([]));
    const test = testAttach();
    await render(
      <InScope scope={{ ...scope, attach: test }}>
        <PendingPromptRow
          prompt={outboxPrompt({
            key: "cmd-1",
            failed: true,
            reason: UPLOAD_LOST,
            withdrawable: true,
          })}
        />
      </InScope>,
    );
    expect(screen.getByText(`not sent · ${UPLOAD_LOST} ·`)).toBeOnTheScreen();
    expect(screen.getByRole("button", { name: "Send it again" })).toBeOnTheScreen();
    expect(
      screen.getByRole("button", { name: "Withdraw the message and its files" }),
    ).toBeOnTheScreen();
  });
});

describe("the files of a sent prompt", () => {
  const shot: Attachment = {
    id: "a1",
    name: "shot.png",
    media_type: "image/png",
    bytes: 2_100_000,
    sha256: "00",
    kind: { kind: "image", width: 3840, height: 2160 },
    origin: { kind: "path", path: "/home/me/shot.png" },
    sent: { media_type: "image/jpeg", bytes: 400_000, width: 2000, height: 1125 },
  };

  it("shows a photo of the terminal as a glyph tile, and a withheld one as a path", async () => {
    const { scope } = testScope(
      run([
        eventItem(1, { ...promptQueued(TURN, "compare @shot.png"), attachments: [shot] }),
        eventItem(2, {
          kind: "attachments_withheld",
          turn_id: TURN,
          attachments: ["a1"],
          reason: "refused",
        }),
      ]),
    );
    await render(
      <InScope scope={scope}>
        <PromptRow turnId={TURN} />
      </InScope>,
    );
    expect(
      screen.getByLabelText("Image, shot.png, 2.1 MB, as a path. The preview is on the machine."),
    ).toBeOnTheScreen();
    expect(screen.getByText("As a path", HIDDEN)).toBeOnTheScreen();
  });
});
