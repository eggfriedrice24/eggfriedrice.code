import { describe, expect, it } from "vitest";

import { FILE_A as ID } from "@/features/conversation/model/outbox-testing";
import { base64Encode } from "@/lib/protocol/bytes";
import { DemoUploads } from "./demo-uploads";

function chunk(offset: number, length: number, name = "IMG_1.jpg", total = 6) {
  return {
    upload_id: ID,
    name,
    total_bytes: total,
    offset,
    data: base64Encode(new Uint8Array(length)),
  };
}

describe("the uploads of the demo machines", () => {
  it("writes a chunk only at the offset that it has, and names the finished upload", () => {
    const uploads = new DemoUploads();
    expect(uploads.put(chunk(0, 4))).toEqual({ received: 4 });
    // A repeated chunk and a resume from an old offset are the same case.
    expect(uploads.put(chunk(0, 4))).toEqual({ received: 4 });
    expect(uploads.put(chunk(4, 2))).toMatchObject({
      received: 6,
      upload: { name: "IMG_1.jpg", kind: { kind: "image" }, sent: { media_type: "image/jpeg" } },
    });
  });

  it("gives a prompt its finished uploads once, and names the missing ones", () => {
    const uploads = new DemoUploads();
    uploads.put(chunk(0, 4, "spec.pdf"));
    expect(uploads.take([ID])).toEqual({ missing: [ID] });
    uploads.put(chunk(4, 2, "spec.pdf"));
    expect(uploads.take([ID])).toMatchObject({
      files: [{ name: "spec.pdf", kind: { kind: "pdf" } }],
    });
    expect(uploads.take([ID])).toEqual({ missing: [ID] });
  });

  it("forgets a discarded upload", () => {
    const uploads = new DemoUploads();
    uploads.put(chunk(0, 6, "notes.txt"));
    uploads.discard(ID);
    expect(uploads.take([ID])).toEqual({ missing: [ID] });
  });
});
