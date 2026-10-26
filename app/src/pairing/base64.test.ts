/// <reference types="vite/client" />
import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { decodeBase64, encodeBase64 } from "./base64";

const enrollParams = import.meta.glob<{ params: { public_key: string } }>(
  "../../../crates/efr-protocol/fixtures/v1/device_enroll_params.json",
  { eager: true, import: "default" },
);

const bytes = (text: string): Uint8Array => Uint8Array.from(text, (char) => char.charCodeAt(0));

/** The key of the fixture: 0x04, then the bytes 1 to 64. */
const FIXTURE_PUBLIC_KEY = Uint8Array.from({ length: 65 }, (_, i) => (i === 0 ? 4 : i));

describe("base64", () => {
  it("gives the known vectors", () => {
    const vectors: [string, string][] = [
      ["", ""],
      ["M", "TQ=="],
      ["Ma", "TWE="],
      ["Man", "TWFu"],
      ["Many", "TWFueQ=="],
      ["Many ", "TWFueSA="],
      ["Many h", "TWFueSBo"],
    ];
    for (const [plain, encoded] of vectors) {
      expect(encodeBase64(bytes(plain))).toBe(encoded);
      expect(decodeBase64(encoded)).toEqual(bytes(plain));
    }
  });

  it("reads the public key of the enroll fixture as 0x04 and 1 to 64", () => {
    const fixture = Object.values(enrollParams)[0];
    expect(fixture).toBeDefined();
    const key = decodeBase64(fixture?.params.public_key ?? "");
    expect(key).toEqual(FIXTURE_PUBLIC_KEY);
    expect(encodeBase64(FIXTURE_PUBLIC_KEY)).toBe(fixture?.params.public_key);
  });

  it("refuses text that is not canonical standard base64", () => {
    for (const text of [
      "Zg=",
      "Zg",
      "Zh==",
      "Zm9=",
      "Z===",
      "====",
      "Zg==Zg==",
      "-_8=",
      "Zm 9v",
      "Zm9v\n",
    ]) {
      expect({ text, bytes: decodeBase64(text) }).toEqual({ text, bytes: undefined });
    }
  });

  it("gives back every byte string", () => {
    fc.assert(
      fc.property(fc.uint8Array({ maxLength: 200 }), (input) => {
        const encoded = encodeBase64(input);
        expect(encoded.length % 4).toBe(0);
        expect(decodeBase64(encoded)).toEqual(input);
      }),
    );
  });
});
