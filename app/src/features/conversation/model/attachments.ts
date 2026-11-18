// The attachments of the phone (spec 7.1, board S-Attach): the files of each draft, the
// files of a prompt after its send, and the previews of the photos.
//
// - A draft holds its files in the order that the user picked them. A photo is
//   "Preparing" while efr-files writes its JPEG with no metadata and its preview; a file
//   needs only its size. The draft keeps them in memory only: the text of a draft goes to
//   the disk, a picked file stays in the caches of iOS.
// - Send takes the files of the draft into the outbox entry (`take`), and the drain
//   uploads them. After the result of `prompt.send` the JPEG and the copy of the picker
//   go (`sent`). The preview stays, by the command id of the prompt, so the sent row
//   shows the photo. iOS can purge the caches: a tile with no preview shows its glyph.
// - The xmark of a draft tile removes it and its files. Withdraw of a prompt on its way
//   removes it, its files and its uploads on efrd (`withdraw`).
//
// Pure TypeScript on TanStack Store. The files, the calls and the disk come in.

import { createStore, type Readable } from "@tanstack/store";

import type { AttachmentLimits, DaemonId, UploadId } from "@/lib/protocol/generated";
import type { KeyValueStorage } from "@/lib/store/key-value";
import type { MachineConnection } from "@/features/machines/model/machine-connection";
import type { EfrFilesNative } from "../../../../modules/efr-files";
import { savedPreviewsSchema } from "./attachments-schemas";
import { fitName } from "./attachments-format";
import type { DraftKey } from "./drafts";
import type { AttachKind, Outbox, OutboxAttachment, OutboxEntry } from "./outbox";

/** The key-value key of the previews. */
export const PREVIEWS_KEY = "efr.previews";

/**
 * The prompts whose previews the app keeps. A transcript shows the newest turns, and iOS
 * can purge the files anyway, so the oldest previews go first.
 */
export const MAX_PREVIEW_PROMPTS = 200;

/** The JPEG quality of an attached photo (spec 7.1). */
export const PHOTO_QUALITY = 0.85;

/** A file that a picker of iOS gave: its copy in the caches of the app. */
export interface PickedFile {
  readonly kind: AttachKind;
  readonly uri: string;
  readonly name: string;
  /** The size that the picker gave; efr-files reads it when the picker gave none. */
  readonly bytes?: number;
}

/** One file of a draft. */
export interface DraftAttachment {
  /** The upload id of the file, made at the pick. */
  readonly id: UploadId;
  readonly kind: AttachKind;
  readonly name: string;
  /** The file to upload: the copy of the picker, then the JPEG of a photo. */
  readonly uri: string;
  /** The size in bytes; undefined while efr-files prepares a photo. */
  readonly bytes: number | undefined;
  /** The preview of a photo, at most 512 px. */
  readonly previewUri?: string;
}

/** The files of each draft, and the previews of the sent photos. */
export interface AttachmentsState {
  readonly drafts: ReadonlyMap<DraftKey, readonly DraftAttachment[]>;
  /** By command id: the preview of each file of the prompt, or null for none. */
  readonly previews: ReadonlyMap<string, SentPreviews>;
}

/** The previews of one sent prompt. */
export interface SentPreviews {
  readonly machine: DaemonId;
  readonly uris: readonly (string | null)[];
}

const NO_FILES: readonly DraftAttachment[] = [];

/** The attachments of a build without the pickers: no draft files and no previews. */
export const NO_ATTACHMENTS: Readable<AttachmentsState> = createStore<AttachmentsState>({
  drafts: new Map(),
  previews: new Map(),
});

/** The files of the draft of `key`, for `useSelector`. */
export function selectDraftFiles(
  key: DraftKey,
): (state: AttachmentsState) => readonly DraftAttachment[] {
  return (state) => state.drafts.get(key) ?? NO_FILES;
}

/** The preview of file `index` of the prompt `commandId`, for `useSelector`. */
export function selectPreview(
  commandId: string,
  index: number,
): (state: AttachmentsState) => string | undefined {
  return (state) => state.previews.get(commandId)?.uris[index] ?? undefined;
}

/**
 * The limits of the attachments of efrd on a ready connection (the capability
 * `attachments`), or undefined: not ready, or an efrd that takes no attachments.
 */
export function attachmentLimits(connection: MachineConnection): AttachmentLimits | undefined {
  const client = connection.client;
  return client.state === "ready"
    ? (client.hello.capabilities?.attachments ?? undefined)
    : undefined;
}

/**
 * A selector of the limits from the connection atom of a supervisor, for `useSelector`.
 * It keeps the limits of the last ready connection while the link opens again, so the
 * plus does not go and come back at each reconnect. Each composer makes its own.
 */
export function createLimitsSelector(): (
  connection: MachineConnection,
) => AttachmentLimits | undefined {
  let last: AttachmentLimits | undefined;
  return (connection) => {
    if (connection.client.state === "ready") {
      last = attachmentLimits(connection);
    }
    return last;
  };
}

/** The name of a photo after efr-files made it a JPEG: `IMG_2041.HEIC` is `IMG_2041.jpg`. */
export function photoName(name: string): string {
  const dot = name.lastIndexOf(".");
  return `${dot > 0 ? name.slice(0, dot) : name}.jpg`;
}

/**
 * `name` as efrd takes it: no `/` and no control character, at most NAME_MAX_BYTES. A
 * name that has nothing left is `file`.
 */
export function uploadName(name: string): string {
  // oxlint-disable-next-line no-control-regex -- efrd refuses each control character.
  const clean = name.replace(/[\u0000-\u001f\u007f/]/g, "_").trim();
  return fitName(clean === "" ? "file" : clean);
}

/** The outbox form of the files of a draft. */
export function outboxFiles(files: readonly DraftAttachment[]): OutboxAttachment[] {
  return files.map((file) => {
    const out: { -readonly [K in keyof OutboxAttachment]: OutboxAttachment[K] } = {
      uploadId: file.id,
      kind: file.kind,
      name: file.name,
      uri: file.uri,
      bytes: file.bytes ?? 0,
      received: 0,
      uploaded: false,
    };
    if (file.previewUri !== undefined) {
      out.previewUri = file.previewUri;
    }
    return out;
  });
}

/** What the attachments need of efr-files. */
export type AttachFiles = Pick<EfrFilesNative, "prepareImage" | "fileInfo" | "remove">;

/** Where the plus of the composer gets a file. */
export type AttachSource = "photos" | "camera" | "files";

/** The pickers of iOS and efr-files (native/attach.ts). */
export interface AttachNative {
  readonly files: EfrFilesNative;
  /** The files that the user picked in `source`, at most `limit`; none on a cancel. */
  pick(source: AttachSource, limit: number): Promise<PickedFile[]>;
}

/** What the attachments need from the app. */
export interface AttachmentsDeps {
  readonly files: AttachFiles;
  readonly outbox: Outbox;
  /** Sends `attachment.discard` to `machine` and does not wait. */
  readonly discard: (machine: DaemonId, uploadId: UploadId) => void;
  /** A new upload id, a v4 UUID. */
  readonly newId: () => string;
  /** The disk of the previews. */
  readonly storage: KeyValueStorage;
  readonly onError: (error: unknown) => void;
}

/** The attachments of all drafts and prompts. */
export interface Attachments {
  readonly store: Readable<AttachmentsState>;
  /**
   * Adds the files that a picker gave to the draft of `key`. A photo becomes a JPEG
   * whose longer edge is at most `maxEdge` pixels.
   */
  add(key: DraftKey, picked: readonly PickedFile[], maxEdge: number): void;
  /** Removes a file of a draft and its copies; a photo in the work stops there. */
  remove(key: DraftKey, id: UploadId): void;
  /**
   * The files of the draft of `key`, and the draft is empty. Undefined, and the draft
   * stays, while a photo is still in the work.
   */
  take(key: DraftKey): readonly DraftAttachment[] | undefined;
  /** efrd took the prompt of `entry`: its uploads are in efrd, so its copies go. */
  sent(entry: OutboxEntry): void;
  /** Takes back a prompt on its way, with its uploads and its files. */
  withdraw(commandId: string): void;
  /** Removes the drafts and the previews of a forgotten machine. */
  removeMachine(machine: DaemonId): void;
}

/** Reads the saved previews, or none when they are broken. */
function loadPreviews(deps: AttachmentsDeps): Map<string, SentPreviews> {
  let json: string | null;
  try {
    json = deps.storage.getItemSync(PREVIEWS_KEY);
  } catch (error) {
    deps.onError(error);
    return new Map();
  }
  if (json === null) {
    return new Map();
  }
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    return new Map();
  }
  const parsed = savedPreviewsSchema.safeParse(value);
  return parsed.success ? new Map(Object.entries(parsed.data)) : new Map();
}

/** Makes the attachments and reads the saved previews. */
export function createAttachments(deps: AttachmentsDeps): Attachments {
  const { files, outbox, onError } = deps;
  const store = createStore<AttachmentsState>({ drafts: new Map(), previews: loadPreviews(deps) });
  let saves: Promise<void> = Promise.resolve();

  const drop = (uri: string | null | undefined): void => {
    if (uri !== undefined && uri !== null) {
      files.remove(uri).catch(onError);
    }
  };

  const savePreviews = (): void => {
    const json = JSON.stringify(Object.fromEntries(store.state.previews));
    saves = saves.then(() => deps.storage.setItem(PREVIEWS_KEY, json)).catch(onError);
  };

  const setDraft = (key: DraftKey, next: readonly DraftAttachment[]): void => {
    store.setState((state) => {
      const drafts = new Map(state.drafts);
      if (next.length === 0) {
        drafts.delete(key);
      } else {
        drafts.set(key, next);
      }
      return { ...state, drafts };
    });
  };

  const draftFile = (key: DraftKey, id: UploadId): DraftAttachment | undefined =>
    store.state.drafts.get(key)?.find((file) => file.id === id);

  // Gives the file `id` of the draft its prepared form, when it is still in the draft.
  const prepared = (key: DraftKey, id: UploadId, change: Partial<DraftAttachment>): boolean => {
    const held = store.state.drafts.get(key) ?? NO_FILES;
    const at = held.findIndex((file) => file.id === id);
    const file = held[at];
    if (file === undefined) {
      return false;
    }
    const next = [...held];
    next[at] = { ...file, ...change };
    setDraft(key, next);
    return true;
  };

  // The JPEG of a photo, or the size of a file that the picker gave none. When the xmark
  // took the tile during the work, the work cleans its own files.
  const prepare = async (
    key: DraftKey,
    id: UploadId,
    picked: PickedFile,
    maxEdge: number,
  ): Promise<void> => {
    if (picked.kind !== "photo") {
      const { bytes } = await files.fileInfo(picked.uri);
      if (!prepared(key, id, { bytes })) {
        drop(picked.uri);
      }
      return;
    }
    const image = await files.prepareImage(picked.uri, maxEdge, PHOTO_QUALITY);
    drop(picked.uri);
    const change: Partial<DraftAttachment> = {
      uri: image.uri,
      bytes: image.bytes,
      previewUri: image.previewUri,
    };
    if (!prepared(key, id, change)) {
      drop(image.uri);
      drop(image.previewUri);
    }
  };

  const removeDraft = (file: DraftAttachment): void => {
    // A photo in the work keeps its input until efr-files ends; `prepare` cleans then.
    if (file.bytes !== undefined) {
      drop(file.uri);
      drop(file.previewUri);
    }
  };

  return {
    store,

    add(key: DraftKey, picked: readonly PickedFile[], maxEdge: number): void {
      const added = picked.map((pick) => {
        const file: DraftAttachment = {
          id: deps.newId(),
          kind: pick.kind,
          name: uploadName(pick.kind === "photo" ? photoName(pick.name) : pick.name),
          uri: pick.uri,
          bytes: pick.kind === "photo" ? undefined : pick.bytes,
        };
        return { file, pick };
      });
      setDraft(key, [
        ...(store.state.drafts.get(key) ?? NO_FILES),
        ...added.map(({ file }) => file),
      ]);
      for (const { file, pick } of added) {
        if (file.bytes !== undefined) {
          continue;
        }
        prepare(key, file.id, pick, maxEdge).catch((error: unknown) => {
          onError(error);
          drop(pick.uri);
          const held = store.state.drafts.get(key) ?? NO_FILES;
          setDraft(
            key,
            held.filter((other) => other.id !== file.id),
          );
        });
      }
    },

    remove(key: DraftKey, id: UploadId): void {
      const file = draftFile(key, id);
      if (file === undefined) {
        return;
      }
      setDraft(
        key,
        (store.state.drafts.get(key) ?? NO_FILES).filter((other) => other.id !== id),
      );
      removeDraft(file);
    },

    take(key: DraftKey): readonly DraftAttachment[] | undefined {
      const held = store.state.drafts.get(key) ?? NO_FILES;
      if (held.some((file) => file.bytes === undefined)) {
        return undefined;
      }
      setDraft(key, NO_FILES);
      return held;
    },

    sent(entry: OutboxEntry): void {
      const sentFiles = entry.attachments;
      if (sentFiles === undefined) {
        return;
      }
      for (const file of sentFiles) {
        drop(file.uri);
      }
      if (!sentFiles.some((file) => file.previewUri !== undefined)) {
        return;
      }
      const previews = new Map(store.state.previews);
      previews.set(entry.commandId, {
        machine: entry.machine,
        uris: sentFiles.map((file) => file.previewUri ?? null),
      });
      for (const [commandId, old] of previews) {
        if (previews.size <= MAX_PREVIEW_PROMPTS) {
          break;
        }
        previews.delete(commandId);
        for (const uri of old.uris) {
          drop(uri);
        }
      }
      store.setState((state) => ({ ...state, previews }));
      savePreviews();
    },

    withdraw(commandId: string): void {
      const entry = outbox.discard(commandId);
      if (entry?.attachments === undefined) {
        return;
      }
      // NOTE: a discard of an upload that efrd does not have is no error.
      for (const file of entry.attachments) {
        deps.discard(entry.machine, file.uploadId);
        drop(file.uri);
        drop(file.previewUri);
      }
    },

    removeMachine(machine: DaemonId): void {
      const prefix = `${machine}:`;
      const state = store.state;
      const drafts = new Map(state.drafts);
      for (const [key, held] of state.drafts) {
        if (key.startsWith(prefix)) {
          drafts.delete(key);
          held.forEach(removeDraft);
        }
      }
      const previews = new Map(state.previews);
      for (const [commandId, sent] of state.previews) {
        if (sent.machine === machine) {
          previews.delete(commandId);
          for (const uri of sent.uris) {
            drop(uri);
          }
        }
      }
      store.setState(() => ({ drafts, previews }));
      if (previews.size !== state.previews.size) {
        savePreviews();
      }
    },
  };
}
