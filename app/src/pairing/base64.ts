// Standard base64 (RFC 4648 section 4, with padding), as efr-protocol's `Base64Bytes`
// writes and reads it: the public key of `device.enroll` and the signature of
// `device.prove`. These functions do not use atob or btoa, because they work on strings of
// UTF-16 units, not on bytes.

const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

const VALUES: ReadonlyMap<string, number> = new Map(
  Array.from(ALPHABET, (char, index) => [char, index]),
);

/** The standard base64 of `bytes`, with padding. */
export function encodeBase64(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const a = bytes[i] ?? 0;
    const b = bytes[i + 1] ?? 0;
    const c = bytes[i + 2] ?? 0;
    const triple = (a << 16) | (b << 8) | c;
    out += ALPHABET.charAt((triple >> 18) & 63);
    out += ALPHABET.charAt((triple >> 12) & 63);
    out += i + 1 < bytes.length ? ALPHABET.charAt((triple >> 6) & 63) : "=";
    out += i + 2 < bytes.length ? ALPHABET.charAt(triple & 63) : "=";
  }
  return out;
}

/**
 * The bytes of the standard base64 `text`, or undefined when `text` is not canonical
 * base64: a length that is not a multiple of 4, a character outside the alphabet, padding
 * in the wrong place, or bits after the last byte that are not zero.
 */
export function decodeBase64(text: string): Uint8Array | undefined {
  if (text.length % 4 !== 0) {
    return undefined;
  }
  const padding = text.endsWith("==") ? 2 : text.endsWith("=") ? 1 : 0;
  const body = text.length - padding;
  const out = new Uint8Array((text.length / 4) * 3 - padding);
  let bits = 0;
  let count = 0;
  let index = 0;
  for (let i = 0; i < body; i++) {
    const value = VALUES.get(text.charAt(i));
    if (value === undefined) {
      return undefined;
    }
    bits = ((bits << 6) | value) & 0xffffff;
    count += 6;
    if (count >= 8) {
      count -= 8;
      out[index++] = (bits >> count) & 0xff;
    }
  }
  // The bits that are left must be zero, so that each byte string has one base64 form.
  if ((bits & ((1 << count) - 1)) !== 0) {
    return undefined;
  }
  return out;
}
