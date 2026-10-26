import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { utf8Length } from "../core/utf8";
import { base64Decode, base64Encode, hex, utf8Bytes } from "./bytes";

function ascii(text: string): Uint8Array {
  return Uint8Array.from(Array.from(text, (char) => char.charCodeAt(0)));
}

describe("utf8Bytes", () => {
  it("encodes one, two, three and four byte characters", () => {
    expect(hex(utf8Bytes("aß日🙂"))).toBe("61c39fe697a5f09f9982");
    expect(hex(utf8Bytes("größe"))).toBe("6772c3b6c39f65");
  });

  it("encodes a lone surrogate as U+FFFD", () => {
    expect(hex(utf8Bytes("a\ud800b\udc00"))).toBe("61efbfbd62efbfbd");
  });

  it("gives as many bytes as utf8Length counts", () => {
    fc.assert(
      fc.property(fc.string({ unit: "binary", maxLength: 200 }), (text) => {
        expect(utf8Bytes(text).length).toBe(utf8Length(text));
      }),
    );
  });
});

describe("base64", () => {
  it("encodes the vectors of RFC 4648", () => {
    // The vectors are the prefixes of "foobar".
    const vectors = ["", "Zg==", "Zm8=", "Zm9v", "Zm9vYg==", "Zm9vYmE=", "Zm9vYmFy"];
    vectors.forEach((encoded, length) => {
      const plain = "foobar".slice(0, length);
      expect(base64Encode(ascii(plain))).toBe(encoded);
      expect(base64Decode(encoded)).toEqual(ascii(plain));
    });
  });

  it("decodes text without padding and refuses text that is not base64", () => {
    expect(base64Decode("Zm8")).toEqual(ascii("foobar".slice(0, 2)));
    for (const text of ["Z", "Zm9v!", "Zm-v", "Zh==", "Zm9=", "Z===", "Zg=", "Zm9v=="]) {
      expect({ text, decoded: base64Decode(text) }).toEqual({ text, decoded: undefined });
    }
  });

  it("decodes what it encodes", () => {
    fc.assert(
      fc.property(fc.uint8Array({ maxLength: 100 }), (bytes) => {
        expect(base64Decode(base64Encode(bytes))).toEqual(bytes);
      }),
    );
  });

  it("encodes the public key of the enroll fixture", () => {
    const point = Uint8Array.from({ length: 65 }, (_, i) => (i === 0 ? 4 : i));
    expect(base64Encode(point)).toBe(
      "BAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0A=",
    );
  });
});
