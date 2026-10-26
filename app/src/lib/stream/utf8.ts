// UTF-8 byte counts of JS strings. efrd counts `offset` in UTF-8 bytes, and a JS string
// counts UTF-16 code units, so a client that compares an offset with `text.length` is
// wrong for any text that is not ASCII. These functions do not use TextEncoder, because
// Hermes support for it is not verified.
//
// A lone surrogate counts 3 bytes, as TextEncoder encodes it (U+FFFD). efrd never sends
// one, because its text is valid UTF-8.

/** The number of bytes of `text` in UTF-8. */
export function utf8Length(text: string): number {
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    const unit = text.charCodeAt(i);
    if (unit < 0x80) {
      bytes += 1;
    } else if (unit < 0x800) {
      bytes += 2;
    } else if (isHighSurrogate(unit) && isLowSurrogate(text.charCodeAt(i + 1))) {
      bytes += 4;
      i++;
    } else {
      bytes += 3;
    }
  }
  return bytes;
}

/**
 * The UTF-16 index in `text` where its first `bytes` UTF-8 bytes end, or `undefined`
 * when `bytes` falls inside a character or past the end of `text`.
 */
export function utf16IndexAtByte(text: string, bytes: number): number | undefined {
  if (bytes < 0) {
    return undefined;
  }
  let seen = 0;
  let i = 0;
  while (seen < bytes && i < text.length) {
    const unit = text.charCodeAt(i);
    if (unit < 0x80) {
      seen += 1;
      i += 1;
    } else if (unit < 0x800) {
      seen += 2;
      i += 1;
    } else if (isHighSurrogate(unit) && isLowSurrogate(text.charCodeAt(i + 1))) {
      seen += 4;
      i += 2;
    } else {
      seen += 3;
      i += 1;
    }
  }
  return seen === bytes ? i : undefined;
}

/** `text` without its first `bytes` UTF-8 bytes, or `undefined` as for `utf16IndexAtByte`. */
export function dropUtf8Prefix(text: string, bytes: number): string | undefined {
  const index = utf16IndexAtByte(text, bytes);
  return index === undefined ? undefined : text.slice(index);
}

/** True when UTF-16 index `index` of `text` does not split a surrogate pair. */
export function isCodePointBoundary(text: string, index: number): boolean {
  if (index <= 0 || index >= text.length) {
    return true;
  }
  return !(isLowSurrogate(text.charCodeAt(index)) && isHighSurrogate(text.charCodeAt(index - 1)));
}

function isHighSurrogate(unit: number): boolean {
  return unit >= 0xd800 && unit <= 0xdbff;
}

function isLowSurrogate(unit: number): boolean {
  return unit >= 0xdc00 && unit <= 0xdfff;
}
