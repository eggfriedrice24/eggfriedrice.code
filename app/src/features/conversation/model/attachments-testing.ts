// Helpers for the tests of the attachments: efr-files with photos that a test prepares by
// hand, pickers that give what the test puts in them, and the attachments of a test on
// them. Only test files import this module.

import { MemoryKeyValue } from "@/lib/store/testing";
import type { EfrFilesNative, FileInfo, PreparedImage } from "../../../../modules/efr-files";
import {
  createAttachments,
  type AttachNative,
  type Attachments,
  type PickedFile,
} from "./attachments";
import type { Outbox } from "./outbox";
import { idMaker, testOutbox } from "./outbox-testing";

/** efr-files that records what the app asks of it. */
export class FakeFiles implements EfrFilesNative {
  readonly removed: string[] = [];
  /** Each Quick Look as `uri title`. */
  readonly looks: string[] = [];
  readonly waiting: { readonly uri: string; done(): void; fail(): void }[] = [];

  prepareImage(uri: string, maxEdge: number): Promise<PreparedImage> {
    return new Promise((resolve, reject) => {
      this.waiting.push({
        uri,
        done: () =>
          resolve({
            uri: `${uri}.jpg`,
            width: maxEdge,
            height: maxEdge / 2,
            bytes: 1_200_000,
            previewUri: `${uri}-preview.jpg`,
          }),
        fail: () => reject(new Error("iOS cannot read it as an image")),
      });
    });
  }

  fileInfo(uri: string): Promise<FileInfo> {
    return Promise.resolve({ name: uri, bytes: 48_000 });
  }

  readChunk(): Promise<string> {
    return Promise.resolve("");
  }

  remove(uri: string): Promise<void> {
    this.removed.push(uri);
    return Promise.resolve();
  }

  quickLook(uri: string, title: string): Promise<void> {
    this.looks.push(`${uri} ${title}`);
    return Promise.resolve();
  }

  /** Ends the work on the photo `uri`: done, or failed when `ok` is false. */
  finish(uri: string, ok = true): void {
    const at = this.waiting.findIndex((held) => held.uri === uri);
    const [held] = this.waiting.splice(at, 1);
    if (ok) {
      held?.done();
    } else {
      held?.fail();
    }
  }
}

/** The attachments of a test, and what they record. */
export interface TestAttach {
  readonly native: AttachNative;
  readonly attachments: Attachments;
  readonly files: FakeFiles;
  readonly outbox: Outbox;
  /** Each discard as `<last letter of the machine>:<last letter of the upload>`. */
  readonly discards: string[];
  readonly errors: unknown[];
  /** What the next pick gives. */
  readonly picks: PickedFile[];
  /** Each pick as `source limit`. */
  readonly picked: string[];
}

/** The attachments on fake files and pickers, with `outbox` and `storage`. */
export function testAttach(
  outbox: Outbox = testOutbox().outbox,
  storage = new MemoryKeyValue(),
): TestAttach {
  const files = new FakeFiles();
  const discards: string[] = [];
  const errors: unknown[] = [];
  const picks: PickedFile[] = [];
  const picked: string[] = [];
  const attachments = createAttachments({
    files,
    outbox,
    discard: (machine, uploadId) => discards.push(`${machine.slice(-1)}:${uploadId.slice(-1)}`),
    newId: idMaker("0f0f0f0f"),
    storage,
    onError: (error) => errors.push(error),
  });
  const native: AttachNative = {
    files,
    pick: (source, limit) => {
      picked.push(`${source} ${limit}`);
      return Promise.resolve(picks.splice(0));
    },
  };
  return { native, attachments, files, outbox, discards, errors, picks, picked };
}
