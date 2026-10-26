// The pairing QR code that `efr device add` shows: an `efr://pair` URL (efr-protocol's
// `PairingQr::url`, docs/protocol.md "Remote devices"):
//
//   efr://pair?v=1&d=<daemon_id>&h=<tailnet IPv4>&p=<port>&f=<fqdn>&t=<token>
//
// The parser is strict, because a QR code can come from anywhere: a daemon id that is not
// a lowercase UUID, a host that is not a Tailscale IPv4 address, a bad port or a short
// token makes the code invalid. A member that this build does not know is skipped, so a
// later efr can add one; a new meaning gets a new `v`.

/** What a pairing code holds. The token is a secret: never log it or show it. */
export interface PairingCode {
  readonly daemonId: string;
  /** The daemon's IPv4 address on the tailnet, in 100.64.0.0/10. */
  readonly host: string;
  readonly port: number;
  /** The MagicDNS name of the daemon's node without a final dot, when efrd knows it. */
  readonly fqdn?: string;
  /** The one-time token, unpadded base64url. */
  readonly token: string;
}

/** The result of a parse: the code, or why the text is not a pairing code. */
export type ParsedPairingCode =
  | { readonly ok: true; readonly code: PairingCode }
  | { readonly ok: false; readonly reason: PairingCodeProblem };

/** Why a text is not a pairing code that this build can use. */
export type PairingCodeProblem =
  /** The text is not an `efr://pair` URL. */
  | "not_efr"
  /** The code is for a later version of the app. */
  | "version"
  /** The URL is an efr pairing code, but a member is absent or not valid. */
  | "malformed";

const PREFIX = "efr://pair?";

/** The version of the payload that this build reads. */
export const PAIRING_CODE_VERSION = "1";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const PORT = /^[1-9][0-9]{0,4}$/;
const OCTET = /^(?:0|[1-9][0-9]{0,2})$/;
const DNS_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const TOKEN = /^[A-Za-z0-9_-]+$/;

/** 16 random bytes in unpadded base64url: efrd's tokens have at least 128 bits. */
const TOKEN_MIN_CHARS = 22;
const TOKEN_MAX_CHARS = 256;

/** True when `text` looks like a pairing code, before the full parse. */
export function isPairingUrl(text: string): boolean {
  return text.startsWith(PREFIX);
}

/** Parses the text of a pairing QR code. */
export function parsePairingUrl(text: string): ParsedPairingCode {
  if (!isPairingUrl(text)) {
    return { ok: false, reason: "not_efr" };
  }
  const members = new Map<string, string>();
  for (const part of text.slice(PREFIX.length).split("&")) {
    const equals = part.indexOf("=");
    if (equals <= 0) {
      return { ok: false, reason: "malformed" };
    }
    const name = part.slice(0, equals);
    const value = percentDecode(part.slice(equals + 1));
    if (value === undefined || members.has(name)) {
      return { ok: false, reason: "malformed" };
    }
    members.set(name, value);
  }
  const version = members.get("v");
  if (version === undefined) {
    return { ok: false, reason: "malformed" };
  }
  if (version !== PAIRING_CODE_VERSION) {
    return { ok: false, reason: "version" };
  }
  const daemonId = members.get("d");
  const host = members.get("h");
  const port = members.get("p");
  const fqdn = members.get("f");
  const token = members.get("t");
  if (
    daemonId === undefined ||
    !UUID.test(daemonId) ||
    host === undefined ||
    !isTailnetIpv4(host) ||
    port === undefined ||
    !PORT.test(port) ||
    Number(port) > 65_535 ||
    (fqdn !== undefined && !isDnsName(fqdn)) ||
    token === undefined ||
    !isToken(token)
  ) {
    return { ok: false, reason: "malformed" };
  }
  // Tailscale writes a MagicDNS name with a final dot; the app keeps it without one.
  const code: PairingCode =
    fqdn === undefined
      ? { daemonId, host, port: Number(port), token }
      : { daemonId, host, port: Number(port), fqdn: fqdn.replace(/\.$/, ""), token };
  return { ok: true, code };
}

/**
 * True for a dotted IPv4 address in 100.64.0.0/10, the range that Tailscale gives its
 * nodes. efrd binds only its tailnet address, so any other host (a loopback or a LAN
 * address) is not from efr.
 */
export function isTailnetIpv4(text: string): boolean {
  const parts = text.split(".");
  if (parts.length !== 4 || !parts.every((part) => OCTET.test(part) && Number(part) <= 255)) {
    return false;
  }
  const [first, second] = parts.map(Number);
  return first === 100 && second !== undefined && second >= 64 && second <= 127;
}

function isDnsName(text: string): boolean {
  const name = text.endsWith(".") ? text.slice(0, -1) : text;
  const labels = name.split(".");
  return name.length <= 253 && labels.length >= 2 && labels.every((label) => DNS_LABEL.test(label));
}

function isToken(text: string): boolean {
  return text.length >= TOKEN_MIN_CHARS && text.length <= TOKEN_MAX_CHARS && TOKEN.test(text);
}

/**
 * Decodes `%XX` escapes of UTF-8 bytes. Gives undefined for a `%` without two hex digits,
 * for bytes that are not UTF-8, and for a `+` or another byte that `PairingQr::url` always
 * escapes.
 */
export function percentDecode(text: string): string | undefined {
  const bytes: number[] = [];
  for (let i = 0; i < text.length; i++) {
    const char = text.charAt(i);
    if (char === "%") {
      const hex = text.slice(i + 1, i + 3);
      if (!/^[0-9A-Fa-f]{2}$/.test(hex)) {
        return undefined;
      }
      bytes.push(Number.parseInt(hex, 16));
      i += 2;
    } else if (/^[A-Za-z0-9._~-]$/.test(char)) {
      bytes.push(char.charCodeAt(0));
    } else {
      return undefined;
    }
  }
  return utf8Decode(bytes);
}

/** Decodes strict UTF-8: no overlong forms, no surrogates, nothing past U+10FFFF. */
function utf8Decode(bytes: readonly number[]): string | undefined {
  let out = "";
  for (let i = 0; i < bytes.length;) {
    const first = bytes[i] ?? 0;
    let length: number;
    let point: number;
    let min: number;
    if (first < 0x80) {
      length = 1;
      point = first;
      min = 0;
    } else if (first >= 0xc2 && first <= 0xdf) {
      length = 2;
      point = first & 0x1f;
      min = 0x80;
    } else if (first >= 0xe0 && first <= 0xef) {
      length = 3;
      point = first & 0x0f;
      min = 0x800;
    } else if (first >= 0xf0 && first <= 0xf4) {
      length = 4;
      point = first & 0x07;
      min = 0x10000;
    } else {
      return undefined;
    }
    if (i + length > bytes.length) {
      return undefined;
    }
    for (let k = 1; k < length; k++) {
      const next = bytes[i + k] ?? 0;
      if ((next & 0xc0) !== 0x80) {
        return undefined;
      }
      point = (point << 6) | (next & 0x3f);
    }
    if (point < min || point > 0x10ffff || (point >= 0xd800 && point <= 0xdfff)) {
      return undefined;
    }
    out += String.fromCodePoint(point);
    i += length;
  }
  return out;
}
