// Bytes for the protocol: UTF-8 encoding and standard base64. These functions do not use
// TextEncoder, btoa or atob, because Hermes support for them is not verified (R4c.4 of
// the research), and the signed message must be the same bytes on every engine.

const BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

const BASE64_VALUES: ReadonlyMap<string, number> = new Map(
  Array.from(BASE64, (char, value) => [char, value] as const),
);

/**
 * The UTF-8 bytes of `text`. A lone surrogate becomes U+FFFD (EF BF BD), as TextEncoder
 * encodes it.
 */
export function utf8Bytes(text: string): Uint8Array {
  const bytes: number[] = [];
  for (let i = 0; i < text.length; i++) {
    let point = text.charCodeAt(i);
    if (point >= 0xd800 && point <= 0xdbff) {
      const low = text.charCodeAt(i + 1);
      if (low >= 0xdc00 && low <= 0xdfff) {
        point = 0x10000 + ((point - 0xd800) << 10) + (low - 0xdc00);
        i++;
      } else {
        point = 0xfffd;
      }
    } else if (point >= 0xdc00 && point <= 0xdfff) {
      point = 0xfffd;
    }
    if (point < 0x80) {
      bytes.push(point);
    } else if (point < 0x800) {
      bytes.push(0xc0 | (point >> 6), 0x80 | (point & 0x3f));
    } else if (point < 0x10000) {
      bytes.push(0xe0 | (point >> 12), 0x80 | ((point >> 6) & 0x3f), 0x80 | (point & 0x3f));
    } else {
      bytes.push(
        0xf0 | (point >> 18),
        0x80 | ((point >> 12) & 0x3f),
        0x80 | ((point >> 6) & 0x3f),
        0x80 | (point & 0x3f),
      );
    }
  }
  return Uint8Array.from(bytes);
}

/** `bytes` as standard base64 with padding, as efr-protocol's `Base64Bytes`. */
export function base64Encode(bytes: Uint8Array): string {
  let text = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const a = bytes[i] ?? 0;
    const b = bytes[i + 1] ?? 0;
    const c = bytes[i + 2] ?? 0;
    const group = (a << 16) | (b << 8) | c;
    text += BASE64.charAt((group >> 18) & 0x3f);
    text += BASE64.charAt((group >> 12) & 0x3f);
    text += i + 1 < bytes.length ? BASE64.charAt((group >> 6) & 0x3f) : "=";
    text += i + 2 < bytes.length ? BASE64.charAt(group & 0x3f) : "=";
  }
  return text;
}

/**
 * The bytes of standard base64 `text`, with or without padding, or `undefined` when
 * `text` is not base64.
 */
export function base64Decode(text: string): Uint8Array | undefined {
  const body = text.endsWith("==")
    ? text.slice(0, -2)
    : text.endsWith("=")
      ? text.slice(0, -1)
      : text;
  const padded = body.length !== text.length;
  if (body.length % 4 === 1 || (padded && text.length % 4 !== 0)) {
    return undefined;
  }
  const bytes: number[] = [];
  let group = 0;
  let bits = 0;
  for (const char of body) {
    const value = BASE64_VALUES.get(char);
    if (value === undefined) {
      return undefined;
    }
    group = (group << 6) | value;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((group >> bits) & 0xff);
    }
  }
  // The bits after the last byte must be zero, so each byte string has one encoding.
  if ((group & ((1 << bits) - 1)) !== 0) {
    return undefined;
  }
  return Uint8Array.from(bytes);
}

/** `bytes` as lowercase hex. */
export function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
