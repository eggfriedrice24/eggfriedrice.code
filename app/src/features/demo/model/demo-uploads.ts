// The uploads of the demo machines (docs/protocol.md, `attachment.put`): the one result
// rule of efrd, `attachment.discard`, and the finished uploads that a `prompt.send` takes.
// The demo keeps only the sizes, never the bytes, and knows the kind of a file by its
// name. The fake efrd of the tests uses the same class, so the rule exists once. Pure.

import { base64Decode } from "@/lib/protocol/bytes";
import type {
  AttachmentKind,
  AttachmentLimits,
  AttachmentPut,
  AttachmentPutResult,
  SentForm,
  UploadInfo,
} from "@/lib/protocol/generated";

/** The limits of the demo machines: those of an efrd with its default config. */
export const DEMO_ATTACHMENT_LIMITS: AttachmentLimits = {
  chunk_bytes: 512 * 1024,
  max_edge_px: 2000,
  max_file_bytes: 20 * 1024 * 1024,
  max_per_message: 10,
};

/** The size of a photo in the demo: efr-files makes no larger one. */
const DEMO_PHOTO = { width: 2000, height: 1500 } as const;

/** The params of `attachment.put` as the demo reads them: each field can be wrong. */
type PutParams = { readonly [K in keyof AttachmentPut]?: unknown };

interface Upload {
  readonly name: string;
  readonly total: number;
  received: number;
}

/** What a demo efrd makes of the name of a finished file. */
function infoOf(upload: Upload): UploadInfo {
  const name = upload.name.toLowerCase();
  let kind: AttachmentKind = { kind: "other" };
  let mediaType = "application/octet-stream";
  if (/\.(jpe?g|png)$/.test(name)) {
    kind = { kind: "image", ...DEMO_PHOTO };
    mediaType = name.endsWith(".png") ? "image/png" : "image/jpeg";
  } else if (name.endsWith(".pdf")) {
    kind = { kind: "pdf" };
    mediaType = "application/pdf";
  } else if (/\.(txt|md|csv|json|ts|rs)$/.test(name)) {
    kind = { kind: "text" };
    mediaType = "text/plain";
  }
  const info: UploadInfo = { name: upload.name, bytes: upload.total, media_type: mediaType, kind };
  if (kind.kind === "image" || kind.kind === "pdf") {
    const sent: SentForm = { media_type: mediaType, bytes: upload.total };
    if (kind.kind === "image") {
      sent.width = DEMO_PHOTO.width;
      sent.height = DEMO_PHOTO.height;
    }
    info.sent = sent;
  }
  return info;
}

/** The uploads of the phone on the demo machines. */
export class DemoUploads {
  protected readonly uploads = new Map<string, Upload>();

  /** One chunk: written only at the offset that the demo has, else only answered. */
  put(params: PutParams): AttachmentPutResult {
    const id = String(params["upload_id"]);
    const total = typeof params["total_bytes"] === "number" ? params["total_bytes"] : 0;
    const upload = this.uploads.get(id) ?? { name: String(params["name"]), total, received: 0 };
    this.uploads.set(id, upload);
    const data = typeof params["data"] === "string" ? params["data"] : "";
    if (params["offset"] === upload.received) {
      upload.received = Math.min(upload.total, upload.received + (base64Decode(data)?.length ?? 0));
    }
    return upload.received < upload.total
      ? { received: upload.received }
      : { received: upload.received, upload: infoOf(upload) };
  }

  discard(id: string): void {
    this.uploads.delete(id);
  }

  /**
   * The finished uploads `ids`, which go away with the prompt that takes them, or the ids
   * of the ones that are not finished, which then stay.
   */
  take(
    ids: readonly string[],
  ): { readonly files: readonly UploadInfo[] } | { readonly missing: readonly string[] } {
    const missing = ids.filter((id) => {
      const upload = this.uploads.get(id);
      return upload === undefined || upload.received < upload.total;
    });
    if (missing.length > 0) {
      return { missing };
    }
    const files: UploadInfo[] = [];
    for (const id of ids) {
      const upload = this.uploads.get(id);
      if (upload !== undefined) {
        files.push(infoOf(upload));
        this.uploads.delete(id);
      }
    }
    return { files };
  }
}
