import { describe, expect, it } from "vitest";

import { flush } from "@/lib/net/testing";
import { MemoryKeyValue } from "@/lib/store/testing";
import {
  MAX_PREVIEW_PROMPTS,
  PREVIEWS_KEY,
  outboxFiles,
  photoName,
  selectDraftFiles,
  selectPreviews,
  uploadName,
  type Attachments,
} from "./attachments";
import { testAttach } from "./attachments-testing";
import { draftKey } from "./drafts";
import type { Outbox, OutboxEntry } from "./outbox";
import { CONVERSATION_A, DESKTOP, LAPTOP, testOutbox } from "./outbox-testing";

const KEY = draftKey(DESKTOP, CONVERSATION_A);

function setup(storage = new MemoryKeyValue()) {
  return testAttach(testOutbox().outbox, storage);
}

function draft(attachments: Attachments) {
  return selectDraftFiles(KEY)(attachments.store.get());
}

describe("the names of an attachment", () => {
  it("names a photo as the JPEG that efr-files writes", () => {
    expect(photoName("IMG_2041.HEIC")).toBe("IMG_2041.jpg");
    expect(photoName("photo")).toBe("photo.jpg");
  });

  it("takes out what efrd refuses and cuts a long name", () => {
    expect(uploadName("a/b\u0007c.txt")).toBe("a_b_c.txt");
    expect(uploadName("   ")).toBe("file");
    expect(uploadName(`${"x".repeat(300)}.pdf`)).toHaveLength(240);
  });
});

describe("the files of a draft", () => {
  it("prepares a photo and gives a file its size, in the order of the pick", async () => {
    const { attachments, files } = setup();
    attachments.add(
      KEY,
      [
        { kind: "photo", uri: "file:///caches/IMG_1.HEIC", name: "IMG_1.HEIC" },
        { kind: "file", uri: "file:///caches/data.csv", name: "data.csv" },
        { kind: "pdf", uri: "file:///caches/a.pdf", name: "a.pdf", bytes: 3_400_000 },
      ],
      2000,
    );
    expect(draft(attachments).map((file) => [file.name, file.bytes])).toEqual([
      ["IMG_1.jpg", undefined],
      ["data.csv", undefined],
      ["a.pdf", 3_400_000],
    ]);
    await flush();
    // A photo in the work holds the send.
    expect(attachments.take(KEY)).toBeUndefined();
    files.finish("file:///caches/IMG_1.HEIC");
    await flush();
    expect(draft(attachments)).toMatchObject([
      {
        name: "IMG_1.jpg",
        uri: "file:///caches/IMG_1.HEIC.jpg",
        bytes: 1_200_000,
        previewUri: "file:///caches/IMG_1.HEIC-preview.jpg",
      },
      { name: "data.csv", bytes: 48_000 },
      { name: "a.pdf", bytes: 3_400_000 },
    ]);
    // The copy of the photo picker goes once the JPEG is there.
    expect(files.removed).toEqual(["file:///caches/IMG_1.HEIC"]);
    const taken = attachments.take(KEY);
    expect(taken).toHaveLength(3);
    expect(draft(attachments)).toEqual([]);
    expect(outboxFiles(taken ?? [])[0]).toMatchObject({ received: 0, uploaded: false });
  });

  it("removes a file and its copies, and cleans a photo that the xmark took during its work", async () => {
    const { attachments, files } = setup();
    attachments.add(
      KEY,
      [
        { kind: "photo", uri: "file:///caches/IMG_1.HEIC", name: "IMG_1.HEIC" },
        { kind: "file", uri: "file:///caches/data.csv", name: "data.csv", bytes: 10 },
      ],
      2000,
    );
    await flush();
    const [photo, csv] = draft(attachments);
    attachments.remove(KEY, csv?.id ?? "");
    attachments.remove(KEY, photo?.id ?? "");
    expect(draft(attachments)).toEqual([]);
    expect(files.removed).toEqual(["file:///caches/data.csv"]);
    files.finish("file:///caches/IMG_1.HEIC");
    await flush();
    expect(files.removed).toEqual([
      "file:///caches/data.csv",
      "file:///caches/IMG_1.HEIC",
      "file:///caches/IMG_1.HEIC.jpg",
      "file:///caches/IMG_1.HEIC-preview.jpg",
    ]);
  });

  it("drops a photo that iOS cannot read and reports why", async () => {
    const { attachments, files, errors } = setup();
    attachments.add(KEY, [{ kind: "photo", uri: "file:///caches/x.png", name: "x.png" }], 2000);
    await flush();
    files.finish("file:///caches/x.png", false);
    await flush();
    expect(draft(attachments)).toEqual([]);
    expect(errors).toHaveLength(1);
    expect(files.removed).toEqual(["file:///caches/x.png"]);
  });
});

/** A prompt in `outbox` with the files of the draft. */
function entryWith(outbox: Outbox, attachments: Attachments): OutboxEntry {
  return outbox.enqueue({
    machine: DESKTOP,
    target: { kind: "conversation", conversationId: CONVERSATION_A },
    text: "look",
    attachments: outboxFiles(attachments.take(KEY) ?? []),
  });
}

describe("the files of a prompt", () => {
  it("removes the uploaded copies after the send and keeps the preview, also on the disk", async () => {
    const storage = new MemoryKeyValue();
    const { attachments, files, outbox } = setup(storage);
    attachments.add(
      KEY,
      [
        { kind: "file", uri: "file:///caches/a.txt", name: "a.txt", bytes: 3 },
        { kind: "photo", uri: "file:///caches/IMG_1.HEIC", name: "IMG_1.HEIC" },
      ],
      2000,
    );
    await flush();
    files.finish("file:///caches/IMG_1.HEIC");
    await flush();
    const entry = entryWith(outbox, attachments);
    files.removed.length = 0;
    attachments.sent(entry);
    expect(files.removed).toEqual(["file:///caches/a.txt", "file:///caches/IMG_1.HEIC.jpg"]);
    const kept = { machine: DESKTOP, uris: [null, "file:///caches/IMG_1.HEIC-preview.jpg"] };
    expect(selectPreviews(entry.commandId)(attachments.store.get())).toEqual(kept);
    await flush();
    expect(storage.getItemSync(PREVIEWS_KEY)).not.toBeNull();
    const again = setup(storage).attachments;
    expect(selectPreviews(entry.commandId)(again.store.get())).toEqual(kept);
  });

  it("keeps the previews of the newest prompts only", async () => {
    const { attachments, files, outbox } = setup();
    const entries: OutboxEntry[] = [];
    for (let i = 0; i <= MAX_PREVIEW_PROMPTS; i++) {
      attachments.add(KEY, [{ kind: "photo", uri: `file:///caches/${i}`, name: `${i}` }], 2000);
      await flush();
      files.finish(`file:///caches/${i}`);
      await flush();
      const entry = entryWith(outbox, attachments);
      entries.push(entry);
      attachments.sent(entry);
    }
    expect(attachments.store.get().previews.size).toBe(MAX_PREVIEW_PROMPTS);
    expect(selectPreviews(entries[0]?.commandId ?? "")(attachments.store.get())).toBeUndefined();
    expect(files.removed).toContain("file:///caches/0-preview.jpg");
  });

  it("withdraws a prompt with its uploads and its files", async () => {
    const { attachments, files, discards, outbox } = setup();
    attachments.add(
      KEY,
      [{ kind: "file", uri: "file:///caches/a.txt", name: "a.txt", bytes: 3 }],
      2000,
    );
    await flush();
    const entry = entryWith(outbox, attachments);
    attachments.withdraw(entry.commandId);
    expect(outbox.store.get().entries).toEqual([]);
    expect(discards).toEqual(["1:1"]);
    expect(files.removed).toEqual(["file:///caches/a.txt"]);
  });

  it("forgets the drafts and the previews of a machine", async () => {
    const storage = new MemoryKeyValue();
    const { attachments, files, outbox } = setup(storage);
    attachments.add(KEY, [{ kind: "photo", uri: "file:///caches/p", name: "p" }], 2000);
    await flush();
    files.finish("file:///caches/p");
    await flush();
    const entry = entryWith(outbox, attachments);
    outbox.succeed(entry.commandId, {
      conversation_id: CONVERSATION_A,
      queued: false,
      seq: 1,
      turn_id: "00000000-0000-4000-8000-0000000000f1",
    });
    attachments.sent(entry);
    const other = draftKey(LAPTOP, CONVERSATION_A);
    attachments.add(other, [{ kind: "file", uri: "file:///caches/l", name: "l", bytes: 1 }], 2000);
    attachments.add(KEY, [{ kind: "file", uri: "file:///caches/d", name: "d", bytes: 1 }], 2000);
    await flush();
    files.removed.length = 0;
    attachments.removeMachine(DESKTOP);
    expect(draft(attachments)).toEqual([]);
    expect(selectDraftFiles(other)(attachments.store.get())).toHaveLength(1);
    expect(attachments.store.get().previews.size).toBe(0);
    expect(files.removed).toEqual(["file:///caches/d", "file:///caches/p-preview.jpg"]);
    await flush();
    expect(storage.getItemSync(PREVIEWS_KEY)).toBe("{}");
  });

  it("removes the files of the prompts on their way to a forgotten machine", async () => {
    const { attachments, files, outbox } = setup();
    attachments.add(
      KEY,
      [{ kind: "file", uri: "file:///caches/a.txt", name: "a.txt", bytes: 3 }],
      2000,
    );
    await flush();
    entryWith(outbox, attachments);
    attachments.removeMachine(LAPTOP);
    expect(files.removed).toEqual([]);
    attachments.removeMachine(DESKTOP);
    expect(files.removed).toEqual(["file:///caches/a.txt"]);
  });
});
