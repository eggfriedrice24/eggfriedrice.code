import { describe, expect, it } from "vitest";

import { base64Decode, base64Encode } from "@/lib/protocol/bytes";
import { DeviceKeyError, deviceKeyStore, isDerSignature, keyFingerprint } from "./device-key";
import { FAKE_SIGNATURE, FakeKeyBinding, FIXTURE_PUBLIC_KEY } from "./testing";

function hexBytes(hex: string): Uint8Array {
  return Uint8Array.from(hex.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
}

/** A DER signature with the integers `r` and `s` as they are on the wire. */
function der(r: number[], s: number[]): Uint8Array {
  const body = [0x02, r.length, ...r, 0x02, s.length, ...s];
  return Uint8Array.from([0x30, body.length, ...body]);
}

describe("deviceKeyStore", () => {
  it("makes, loads and removes a key under its alias", async () => {
    const binding = new FakeKeyBinding();
    const keys = deviceKeyStore(binding);
    expect(keys.hardwareBacked).toBe(true);
    const made = await keys.create("device-1");
    expect(made.alias).toBe("device-1");
    expect(made.publicKey.length).toBe(65);
    expect(made.publicKey[0]).toBe(4);
    const loaded = await keys.load("device-1");
    expect(loaded?.publicKey).toEqual(made.publicKey);
    await keys.remove("device-1");
    expect(await keys.load("device-1")).toBeNull();
  });

  it("signs the message bytes and gives the DER signature", async () => {
    const binding = new FakeKeyBinding();
    const key = await deviceKeyStore(binding).create("device-1");
    const message = Uint8Array.from([0, 1, 2, 250, 255]);
    expect(await key.sign(message)).toEqual(FAKE_SIGNATURE);
    expect(binding.signed).toEqual([{ alias: "device-1", message: base64Encode(message) }]);
  });

  it.each([
    ["64 bytes", base64Encode(FIXTURE_PUBLIC_KEY.subarray(0, 64))],
    ["a compressed prefix", base64Encode(Uint8Array.from([2, ...FIXTURE_PUBLIC_KEY.subarray(1)]))],
    ["66 bytes", base64Encode(Uint8Array.from([...FIXTURE_PUBLIC_KEY, 0]))],
    ["text that is not base64", "not base64"],
  ])("refuses a public key of %s", async (_, bad) => {
    const binding = new FakeKeyBinding();
    binding.createKey = () => Promise.resolve(bad);
    await expect(deviceKeyStore(binding).create("device-1")).rejects.toBeInstanceOf(DeviceKeyError);
  });

  it("refuses a signature that is not DER", async () => {
    const binding = new FakeKeyBinding();
    const key = await deviceKeyStore(binding).create("device-1");
    binding.sign = () => Promise.resolve(base64Encode(new Uint8Array(64)));
    await expect(key.sign(new Uint8Array(1))).rejects.toBeInstanceOf(DeviceKeyError);
  });

  it.each(["", "a/b", "a b", "x".repeat(65), "\u00e9"])(
    "refuses the alias %j, which the native module would refuse",
    async (alias) => {
      const keys = deviceKeyStore(new FakeKeyBinding());
      await expect(keys.create(alias)).rejects.toBeInstanceOf(DeviceKeyError);
    },
  );
});

describe("isDerSignature", () => {
  it("takes the signatures of P-256 in their minimal form", () => {
    const high = [0x80, ...Array.from({ length: 31 }, () => 1)];
    expect(isDerSignature(FAKE_SIGNATURE)).toBe(true);
    expect(isDerSignature(der([0x00, ...high], [0x00, ...high]))).toBe(true);
    expect(isDerSignature(der([0x01], [0x7f]))).toBe(true);
    // 71 bytes: r needs a zero byte before it, s does not.
    expect(
      isDerSignature(
        hexBytes(
          "3045022100c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5022045b0" +
            "3fd8b6d0b0d3e1b2b5c63aee17d7bbd3e0b1b5f38c3a5b3b0e2f7c6f1d02",
        ),
      ),
    ).toBe(true);
  });

  it("refuses a value that is not a SEQUENCE of two minimal positive INTEGERs", () => {
    const high = [0x80, ...Array.from({ length: 31 }, () => 1)];
    const bad = [
      new Uint8Array(0),
      new Uint8Array(64),
      der(high, [0x01]),
      der([0x00, 0x01], [0x01]),
      der([], [0x01]),
      der(
        Array.from({ length: 34 }, () => 1),
        [0x01],
      ),
      Uint8Array.from([...der([0x01], [0x01]), 0x00]),
      Uint8Array.from([0x31, ...der([0x01], [0x01]).subarray(1)]),
      Uint8Array.from([0x30, 0x06, 0x02, 0x01, 0x01, 0x03, 0x01, 0x01]),
      Uint8Array.from([0x30, 0x07, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01]),
    ];
    for (const bytes of bad) {
      expect({ der: base64Encode(bytes), valid: isDerSignature(bytes) }).toEqual({
        der: base64Encode(bytes),
        valid: false,
      });
    }
  });
});

describe("keyFingerprint", () => {
  it("gives the first 8 bytes of the SHA-256 in hex", () => {
    // `printf test | sha256sum`, the digest in the fixture device_pair_start_item_pending.json.
    const digest = hexBytes("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08");
    expect(keyFingerprint(digest)).toBe("SHA256:9f86d081884c7d65");
  });

  it("gives the fingerprint of the fixture key", () => {
    // The SHA-256 of the 65 bytes of FIXTURE_PUBLIC_KEY, from sha256sum.
    const digest = hexBytes("0ed3a6ab957ff6f59a9630a473d31a7d04fcb46548b89016196980c7e96584f6");
    expect(base64Decode(base64Encode(FIXTURE_PUBLIC_KEY))?.length).toBe(65);
    expect(keyFingerprint(digest)).toBe("SHA256:0ed3a6ab957ff6f5");
  });

  it("refuses a digest of the wrong length", () => {
    expect(() => keyFingerprint(new Uint8Array(31))).toThrow(DeviceKeyError);
  });
});
