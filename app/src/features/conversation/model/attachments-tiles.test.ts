import { describe, expect, it } from "vitest";

import type { Attachment, AttachmentLimits } from "@/lib/protocol/generated";
import type { PromptView } from "@/lib/stream/reducer";
import type { DraftAttachment } from "./attachments";
import { captionLine } from "./attachments-format";
import {
  draftTiles,
  overLimit,
  pendingTiles,
  promptTiles,
  sameTiles,
  tilesReady,
  type TileView,
} from "./attachments-tiles";
import type { OutboxEntry } from "./outbox";
import { CONVERSATION_A, DESKTOP, outboxFile } from "./outbox-testing";

const LIMITS: AttachmentLimits = {
  chunk_bytes: 524_288,
  max_edge_px: 2000,
  max_file_bytes: 20 * 1024 * 1024,
  max_per_message: 10,
};

const ID = "0f0f0f0f-0000-4000-8000-00000000000a";

function draftFile(change: Partial<DraftAttachment>): DraftAttachment {
  return {
    id: ID,
    kind: "file",
    name: "logs.zip",
    uri: "file:///caches/logs.zip",
    bytes: 10,
    ...change,
  };
}

function entry(change: Partial<OutboxEntry>): OutboxEntry {
  return {
    commandId: "00000000-0000-4000-8000-000000000001",
    machine: DESKTOP,
    target: { kind: "conversation", conversationId: CONVERSATION_A },
    text: "look",
    createdAt: 0,
    order: 1,
    attempts: 0,
    state: "sending",
    retryAt: 0,
    error: null,
    sent: null,
    attachments: [{ ...outboxFile(ID, 1_000), received: 400 }],
    ...change,
  };
}

function captions(tiles: readonly TileView[]): string[] {
  return tiles.map(captionLine);
}

describe("the tiles of a draft", () => {
  it("shows a photo in the work, a size, and a file over the limit, which holds the send", () => {
    const tiles = draftTiles(
      [
        draftFile({ kind: "photo", name: "IMG_1.jpg", bytes: undefined }),
        draftFile({ bytes: 48_000 }),
        draftFile({ bytes: 34_000_000 }),
      ],
      LIMITS,
    );
    expect(captions(tiles)).toEqual(["Preparing", "48 KB", "34 MB"]);
    expect(overLimit(tiles)?.bytes).toBe(34_000_000);
    expect(tilesReady(tiles)).toBe(false);
    expect(tilesReady(draftTiles([draftFile({ bytes: 48_000 })], LIMITS))).toBe(true);
  });
});

/** The size lines of the tiles of an entry with `change`. */
function line(change: Partial<OutboxEntry>): string[] {
  return captions(pendingTiles(entry(change)));
}

/** No preview for any file. */
function none(): undefined {
  return undefined;
}

describe("the tiles of a prompt on its way", () => {
  it("puts the state of the upload in the size line", () => {
    expect(line({})).toEqual(["40%"]);
    expect(line({ state: "pending" })).toEqual(["40%"]);
    expect(line({ state: "pending", error: { code: "connection", message: "gone" } })).toEqual([
      "Waiting",
    ]);
    expect(line({ reuploads: 1 })).toEqual(["Uploading again"]);
    expect(line({ state: "failed", error: { code: "upload", message: "lost" } })).toEqual([
      "Not sent",
    ]);
    expect(
      line({ attachments: [{ ...outboxFile(ID, 1_000), received: 1_000, uploaded: true }] }),
    ).toEqual(["1.0 KB"]);
  });

  it("compares two rows by what they show", () => {
    const a = pendingTiles(entry({}));
    expect(sameTiles(a, pendingTiles(entry({})))).toBe(true);
    expect(
      sameTiles(
        a,
        pendingTiles(entry({ attachments: [{ ...outboxFile(ID, 1_000), received: 500 }] })),
      ),
    ).toBe(false);
  });
});

const shot: Attachment = {
  id: "a1",
  name: "IMG_1.jpg",
  media_type: "image/jpeg",
  bytes: 1_200_000,
  sha256: "00",
  kind: { kind: "image", width: 2000, height: 1500 },
  origin: { kind: "phone" },
  sent: { media_type: "image/jpeg", bytes: 1_200_000, width: 2000, height: 1500 },
};

function prompt(attachments: readonly Attachment[], unread?: readonly string[]): PromptView {
  const view: { -readonly [K in keyof PromptView]: PromptView[K] } = {
    text: "look",
    origin: "phone",
    commandId: "c1",
    at: undefined,
    attachments,
  };
  if (unread !== undefined) {
    view.unread = unread;
  }
  return view;
}

describe("the tiles of a prompt of the transcript", () => {
  it("shows the preview of a photo of this phone, and the glyph of a file from the terminal", () => {
    const terminal: Attachment = { ...shot, id: "a2", origin: { kind: "path", path: "/s.png" } };
    const tiles = promptTiles(prompt([shot, terminal]), new Set(), (index) =>
      index === 0 ? "file:///caches/a-preview.jpg" : "file:///caches/b-preview.jpg",
    );
    expect(tiles.map((tile) => [tile.kind, tile.previewUri])).toEqual([
      ["photo", "file:///caches/a-preview.jpg"],
      ["image", undefined],
    ]);
  });

  it("marks an attachment that the model gets only as a path", () => {
    const pdf: Attachment = {
      ...shot,
      id: "a2",
      name: "manual.pdf",
      kind: { kind: "pdf" },
      sent: null,
    };
    const text: Attachment = {
      ...shot,
      id: "a4",
      name: "data.csv",
      kind: { kind: "text" },
      sent: null,
    };
    expect(captions(promptTiles(prompt([shot, pdf, text]), new Set(), none))).toEqual([
      "1.2 MB",
      "As a path",
      "1.2 MB",
    ]);
    expect(captions(promptTiles(prompt([shot], ["a1"]), new Set(), none))).toEqual(["As a path"]);
    expect(captions(promptTiles(prompt([shot]), new Set(["a1"]), none))).toEqual(["As a path"]);
  });
});
