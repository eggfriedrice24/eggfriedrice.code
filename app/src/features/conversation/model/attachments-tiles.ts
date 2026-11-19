// The tiles of the attachment row (board S-Attach) in each place: the draft, a prompt on
// its way (the outbox) and a prompt of the transcript. A tile is one attachment with the
// state that its size line shows; the composer and the rows draw the same tile.
//
// - Draft: "Preparing" while efr-files writes the JPEG; a file over the limit of efrd
//   shows its size in state_error, and the draft cannot send it.
// - On its way: the percent of the upload; "Waiting" while the link is down;
//   "Uploading again" after efrd lost the upload; "Not sent" when the prompt failed.
// - Transcript: the size, or "As a path" when the model got the path and not the content:
//   the turn's model reads no such file (`unread_attachments`), efrd withheld it, or an
//   image or a PDF went with no content form (`sent`).
//
// Pure TypeScript.

import type { Attachment, AttachmentLimits } from "@/lib/protocol/generated";
import type { PromptView } from "@/lib/stream/reducer";
import { uploadPercent, type TileKind, type TileState, type TileText } from "./attachments-format";
import type { DraftAttachment } from "./attachments";
import type { ComposerTrailing } from "./composer-state";
import type { OutboxEntry } from "./outbox";

/** One tile of the row. */
export interface TileView extends TileText {
  /** The key of the tile in its row: the upload id or the attachment id. */
  readonly key: string;
  /** The photo of the tile; none: the tile shows the glyph of its kind. */
  readonly previewUri: string | undefined;
}

const SIZE: TileState = { kind: "size" };
const PREPARING: TileState = { kind: "preparing" };
const WAITING: TileState = { kind: "waiting" };
const NOT_SENT: TileState = { kind: "not_sent" };
const AS_PATH: TileState = { kind: "as_path" };

/** The tiles of a draft; `limits` are those of efrd. */
export function draftTiles(
  files: readonly DraftAttachment[],
  limits: AttachmentLimits | undefined,
): TileView[] {
  return files.map((file) => {
    let state = SIZE;
    if (file.bytes === undefined) {
      state = PREPARING;
    } else if (limits !== undefined && file.bytes > limits.max_file_bytes) {
      state = { kind: "over_limit", limit: limits.max_file_bytes };
    }
    // Before efr-files ends, the tile shows the copy of the photo picker.
    return {
      key: file.id,
      kind: file.kind,
      name: file.name,
      bytes: file.bytes,
      state,
      previewUri: file.previewUri ?? (file.kind === "photo" ? file.uri : undefined),
    };
  });
}

/** The first file of a draft over the limit of efrd, for the line under the row. */
export function overLimit(tiles: readonly TileView[]): TileView | undefined {
  return tiles.find((held) => held.state.kind === "over_limit");
}

/** True when a draft with `tiles` can go: no photo in the work, no file over the limit. */
export function tilesReady(tiles: readonly TileView[]): boolean {
  return tiles.every((held) => held.state.kind === "size");
}

/**
 * The trailing action of a composer whose draft holds `tiles`. While a file is not ready,
 * nothing goes. While a turn runs, the prompt can only queue: a steer takes no file.
 */
export function trailingWithFiles(
  trailing: ComposerTrailing,
  tiles: readonly TileView[],
): ComposerTrailing {
  if (tiles.length === 0) {
    return trailing;
  }
  const ready = tilesReady(tiles);
  if (trailing.kind === "send") {
    return trailing.enabled && !ready ? { kind: "send", enabled: false } : trailing;
  }
  if (trailing.kind === "stop" && trailing.capsule !== null) {
    return { kind: "stop", capsule: ready ? "queue" : null };
  }
  return trailing;
}

/** The tiles of a prompt on its way. */
export function pendingTiles(entry: OutboxEntry): TileView[] {
  return (entry.attachments ?? []).map((file) => {
    const percent = uploadPercent(file.received, file.bytes);
    let state: TileState;
    if (entry.state === "failed") {
      state = NOT_SENT;
    } else if (file.uploaded || entry.state === "sent") {
      state = SIZE;
    } else if ((entry.reuploads ?? 0) > 0) {
      state = { kind: "again", percent };
    } else if (entry.state === "pending" && entry.error !== null) {
      // The link went down during the send: the upload goes on at the next connection.
      state = WAITING;
    } else {
      state = { kind: "uploading", percent };
    }
    return {
      key: file.uploadId,
      kind: file.kind,
      name: file.name,
      bytes: file.bytes,
      state,
      previewUri: file.previewUri,
    };
  });
}

/**
 * The tiles of a prompt of the transcript. `withheld` holds the attachments that efrd
 * sends as a path from now on; `preview` gives the preview that this phone kept of file
 * `index`.
 */
export function promptTiles(
  prompt: PromptView,
  withheld: ReadonlySet<string>,
  preview: (index: number) => string | undefined,
): TileView[] {
  const files = prompt.attachments ?? [];
  if (files.length === 0) {
    return [];
  }
  // A prompt holds at most `max_per_message` files, so a scan of the list is cheap.
  const unread = prompt.unread ?? [];
  return files.map((file, index) => {
    const asPath = unread.includes(file.id) || withheld.has(file.id) || noContent(file);
    const kind = tileKind(file);
    return {
      key: file.id,
      kind,
      name: file.name,
      bytes: file.bytes,
      state: asPath ? AS_PATH : SIZE,
      previewUri: kind === "photo" ? preview(index) : undefined,
    };
  });
}

/** True when the tiles of `a` and `b` show the same. */
export function sameTiles(a: readonly TileView[], b: readonly TileView[]): boolean {
  return (
    a === b ||
    (a.length === b.length &&
      a.every((held, i) => {
        const other = b[i];
        return (
          other !== undefined &&
          other.key === held.key &&
          other.kind === held.kind &&
          other.name === held.name &&
          other.bytes === held.bytes &&
          other.previewUri === held.previewUri &&
          sameState(other.state, held.state)
        );
      }))
  );
}

function sameState(a: TileState, b: TileState): boolean {
  if (a.kind !== b.kind) {
    return false;
  }
  if (
    (a.kind === "uploading" || a.kind === "again") &&
    (b.kind === "uploading" || b.kind === "again")
  ) {
    return a.percent === b.percent;
  }
  if (a.kind === "over_limit" && b.kind === "over_limit") {
    return a.limit === b.limit;
  }
  return true;
}

/** An image or a PDF that went to the model with no content form. */
function noContent(file: Attachment): boolean {
  const kind = file.kind.kind;
  return (kind === "image" || kind === "pdf") && (file.sent === undefined || file.sent === null);
}

/** The tile kind of an attachment of the transcript: a photo only from this phone. */
function tileKind(file: Attachment): TileKind {
  switch (file.kind.kind) {
    case "image":
      return file.origin.kind === "phone" ? "photo" : "image";
    case "pdf":
      return "pdf";
    default:
      return "file";
  }
}
