// The texts of an attachment tile (board S-Attach): the caption under the tile, its
// VoiceOver label and the lines about the limits. Each tile has the same caption, a
// draft tile, a tile of a prompt on its way and a tile of the transcript:
//
// - Line 1 is the name, cut by `fitName` before the upload, as efr-protocol cuts it.
// - Line 2 is the size, as `byte_size` of efr-stdx writes it, so the phone and the
//   terminal say the same size. A state takes its place while it lasts: "Preparing",
//   "40%", "Waiting", "Uploading again", "Not sent", "As a path".
//
// Pure TypeScript.

import { utf8Length } from "@/lib/stream/utf8";

import { byteSize } from "./format";

/** The longest name of an attachment in UTF-8 bytes (`NAME_MAX_BYTES` of efr-protocol). */
export const NAME_MAX_BYTES = 240;

/** The longest extension, with its dot, that `fitName` keeps (efr-protocol). */
export const KEPT_EXTENSION_BYTES = 16;

/**
 * `name` cut to at most NAME_MAX_BYTES bytes, by the rule of `fit_name` of efr-protocol:
 * the stem is cut at a character border, and an extension of at most
 * KEPT_EXTENSION_BYTES stays. efrd refuses a longer name, so the app cuts it first.
 */
export function fitName(name: string): string {
  if (utf8Length(name) <= NAME_MAX_BYTES) {
    return name;
  }
  const dot = name.lastIndexOf(".");
  const extension =
    dot > 0 && utf8Length(name.slice(dot)) <= KEPT_EXTENSION_BYTES ? name.slice(dot) : "";
  const stem = name.slice(0, name.length - extension.length);
  const room = NAME_MAX_BYTES - utf8Length(extension);
  let kept = 0;
  let bytes = 0;
  for (const char of stem) {
    const size = utf8Length(char);
    if (bytes + size > room) {
      break;
    }
    bytes += size;
    kept += char.length;
  }
  return `${stem.slice(0, kept)}${extension}`;
}

/** What a tile shows: a photo of this phone, an image from elsewhere, a PDF or a file. */
export type TileKind = "photo" | "image" | "pdf" | "file";

/** The word of a kind, first in the VoiceOver label. */
const KIND_WORDS: Readonly<Record<TileKind, string>> = {
  photo: "Photo",
  image: "Image",
  pdf: "PDF",
  file: "File",
};

/** What takes the place of the size line, or the size itself. */
export type TileState =
  | { readonly kind: "size" }
  /** efr-files makes the JPEG of a photo. */
  | { readonly kind: "preparing" }
  /** The upload runs, or waits for its turn at 0%. */
  | { readonly kind: "uploading"; readonly percent: number }
  /** iOS stopped the app or the link is down: the upload goes on at the next connection. */
  | { readonly kind: "waiting" }
  /** efrd lost the upload, and the drain sends it again from 0. */
  | { readonly kind: "again"; readonly percent: number }
  | { readonly kind: "not_sent" }
  /** The model gets the path of the file, not its content. */
  | { readonly kind: "as_path" }
  /** The file is larger than the limit of efrd: the draft cannot send it. */
  | { readonly kind: "over_limit"; readonly limit: number };

/** One tile, as its texts need it. */
export interface TileText {
  readonly kind: TileKind;
  readonly name: string;
  /** The size in bytes; undefined while efr-files prepares a photo. */
  readonly bytes: number | undefined;
  readonly state: TileState;
}

/** The percent of an upload, rounded down. */
export function uploadPercent(received: number, bytes: number): number {
  return bytes <= 0 ? 0 : Math.min(100, Math.floor((received * 100) / bytes));
}

/** Line 2 of the caption. */
export function captionLine(tile: TileText): string {
  const state = tile.state;
  switch (state.kind) {
    case "preparing":
      return "Preparing";
    case "uploading":
      return `${state.percent}%`;
    case "waiting":
      return "Waiting";
    case "again":
      return "Uploading again";
    case "not_sent":
      return "Not sent";
    case "as_path":
      return "As a path";
    default:
      return tile.bytes === undefined ? "" : byteSize(tile.bytes);
  }
}

/** The line under the row of a draft with a file over the limit. */
export function overLimitLine(name: string, bytes: number, limit: number): string {
  return `${name} is ${byteSize(bytes)}. The limit is ${byteSize(limit)}.`;
}

/** The sentence of a prompt whose upload failed after its last try. */
export const UPLOAD_LOST = "The upload did not arrive.";

/**
 * The VoiceOver label of a tile: the kind, the name, the size and the state, such as
 * "Photo, IMG_2044.jpg, 2.4 MB, uploading 40 percent". An image from another place says
 * that its picture is on the machine.
 */
export function tileLabel(tile: TileText): string {
  const label = stateLabel(tile);
  return tile.kind === "image" ? `${label}. The preview is on the machine.` : label;
}

function stateLabel(tile: TileText): string {
  const parts = [KIND_WORDS[tile.kind], tile.name];
  const size = tile.bytes === undefined ? undefined : byteSize(tile.bytes);
  const state = tile.state;
  if (state.kind === "preparing") {
    return [...parts, "preparing"].join(", ");
  }
  if (size !== undefined) {
    parts.push(size);
  }
  switch (state.kind) {
    case "uploading":
      return [...parts, `uploading ${state.percent} percent`].join(", ");
    case "waiting":
      return [...parts, "waiting to upload"].join(", ");
    case "again":
      return [...parts, "uploading again", `${state.percent} percent`].join(", ");
    case "not_sent":
      return `${[...parts, "not sent"].join(", ")}. ${UPLOAD_LOST}`;
    case "as_path":
      return [...parts, "as a path"].join(", ");
    case "over_limit":
      return `${parts.join(", ")}. The limit is ${byteSize(state.limit)}.`;
    default:
      return parts.join(", ");
  }
}

/** The label of the xmark of a draft tile. */
export function removeLabel(name: string): string {
  return `Remove ${name}`;
}

/** Why efrd sends attachments as a path from now on (`WithheldReason` of efr-protocol). */
export type WithheldWhy = "budget" | "refused" | "unknown";

/** The quiet row of an `attachments_withheld` event: "Not sent: shot.png. The provider refused it." */
export function withheldLine(names: readonly string[], reason: WithheldWhy): string {
  const list = `Not sent: ${names.join(", ")}.`;
  const them = names.length === 1 ? "it" : "them";
  switch (reason) {
    case "refused":
      return `${list} The provider refused ${them}.`;
    case "budget":
      return `${list} The request had no room for ${them}.`;
    default:
      return list;
  }
}
