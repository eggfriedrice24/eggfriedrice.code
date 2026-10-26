/// <reference types="vite/client" />
import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import { isTailnetIpv4, parsePairingUrl, percentDecode } from "./pairing-url";
import type { PairingCode } from "./pairing-url";

interface QrFixture {
  readonly qr: {
    readonly daemon_id: string;
    readonly host: string;
    readonly port: number;
    readonly fqdn?: string;
    readonly token: string;
  };
  readonly url: string;
}

// The vector of efr-protocol: a payload and the URL that `PairingQr::url` gives.
const qrFixture = Object.values(
  import.meta.glob<QrFixture>(
    "../../../../../crates/efr-protocol/fixtures/v1/pairing_qr_url.json",
    {
      eager: true,
      import: "default",
    },
  ),
)[0];

const DAEMON = "019a9b1c-3d00-7a10-8b20-000000000007";
const TOKEN = "q83vASNFZ4mrze8BI0VniQ";

/** The URL as `PairingQr::url` writes it (crates/efr-protocol/src/device.rs). */
function pairingUrl(code: PairingCode): string {
  let text = `efr://pair?v=1&d=${percentEncode(code.daemonId)}&h=${percentEncode(code.host)}`;
  text += `&p=${code.port}`;
  if (code.fqdn !== undefined) {
    text += `&f=${percentEncode(code.fqdn)}`;
  }
  return `${text}&t=${percentEncode(code.token)}`;
}

/** The percent-encoding of `PairingQr::url`: every byte outside the unreserved set. */
function percentEncode(value: string): string {
  return Array.from(new TextEncoderLike().encode(value), (byte) =>
    /[A-Za-z0-9._~-]/.test(String.fromCharCode(byte))
      ? String.fromCharCode(byte)
      : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`,
  ).join("");
}

/** UTF-8 bytes of a string, without TextEncoder (not in the lib of this project). */
class TextEncoderLike {
  encode(text: string): number[] {
    const out: number[] = [];
    for (const char of text) {
      const point = char.codePointAt(0) ?? 0;
      if (point < 0x80) {
        out.push(point);
      } else if (point < 0x800) {
        out.push(0xc0 | (point >> 6), 0x80 | (point & 63));
      } else if (point < 0x10000) {
        out.push(0xe0 | (point >> 12), 0x80 | ((point >> 6) & 63), 0x80 | (point & 63));
      } else {
        out.push(
          0xf0 | (point >> 18),
          0x80 | ((point >> 12) & 63),
          0x80 | ((point >> 6) & 63),
          0x80 | (point & 63),
        );
      }
    }
    return out;
  }
}

function url(members: string): string {
  return `efr://pair?${members}`;
}

const VALID = `v=1&d=${DAEMON}&h=100.64.0.1&p=7878&t=${TOKEN}`;

describe("parsePairingUrl", () => {
  it("reads the URL of the efr-protocol vector", () => {
    expect(qrFixture).toBeDefined();
    const parsed = parsePairingUrl(qrFixture?.url ?? "");
    expect(parsed).toEqual({
      ok: true,
      code: {
        daemonId: qrFixture?.qr.daemon_id,
        host: qrFixture?.qr.host,
        port: qrFixture?.qr.port,
        fqdn: qrFixture?.qr.fqdn,
        token: qrFixture?.qr.token,
      },
    });
  });

  it("reads a code without a MagicDNS name", () => {
    expect(parsePairingUrl(url(VALID))).toEqual({
      ok: true,
      code: { daemonId: DAEMON, host: "100.64.0.1", port: 7878, token: TOKEN },
    });
  });

  it("drops the final dot of a MagicDNS name", () => {
    const parsed = parsePairingUrl(url(`${VALID}&f=desk.tail1234.ts.net.`));
    expect(parsed.ok && parsed.code.fqdn).toBe("desk.tail1234.ts.net");
  });

  it("reads the members in any order and skips a member that it does not know", () => {
    const parsed = parsePairingUrl(url(`t=${TOKEN}&x=later&p=7878&h=100.64.0.1&d=${DAEMON}&v=1`));
    expect(parsed).toEqual({
      ok: true,
      code: { daemonId: DAEMON, host: "100.64.0.1", port: 7878, token: TOKEN },
    });
  });

  it("tells a text that is not an efr code from a damaged one", () => {
    expect(parsePairingUrl("https://example.com/")).toEqual({ ok: false, reason: "not_efr" });
    expect(parsePairingUrl("EFR://pair?v=1")).toEqual({ ok: false, reason: "not_efr" });
    expect(parsePairingUrl(url(VALID.replace("v=1", "v=2")))).toEqual({
      ok: false,
      reason: "version",
    });
    expect(parsePairingUrl(url(VALID.replace("v=1&", "")))).toEqual({
      ok: false,
      reason: "malformed",
    });
  });

  it("refuses a code with a member that is absent or not valid", () => {
    const bad = [
      VALID.replace(`&t=${TOKEN}`, ""),
      VALID.replace(`d=${DAEMON}`, `d=${DAEMON.toUpperCase()}`),
      VALID.replace(`d=${DAEMON}`, "d=desk"),
      VALID.replace("h=100.64.0.1", "h=127.0.0.1"),
      VALID.replace("h=100.64.0.1", "h=192.168.1.2"),
      VALID.replace("h=100.64.0.1", "h=100.128.0.1"),
      VALID.replace("h=100.64.0.1", "h=100.064.0.1"),
      VALID.replace("h=100.64.0.1", "h=desk.tail1234.ts.net"),
      VALID.replace("p=7878", "p=0"),
      VALID.replace("p=7878", "p=65536"),
      VALID.replace("p=7878", "p=07878"),
      VALID.replace("p=7878", "p=-1"),
      VALID.replace(`t=${TOKEN}`, "t=short"),
      VALID.replace(`t=${TOKEN}`, `t=${TOKEN}%3D`),
      VALID.replace(`t=${TOKEN}`, `t=${TOKEN.slice(0, 10)}+${TOKEN.slice(10)}`),
      `${VALID}&f=`,
      `${VALID}&f=desk`,
      `${VALID}&f=Desk.ts.net`,
      `${VALID}&t=${TOKEN}`,
      `${VALID}&`,
      `${VALID}&novalue`,
      `${VALID}&=x`,
      VALID.replace(`t=${TOKEN}`, `t=${TOKEN}%G1`),
      VALID.replace(`t=${TOKEN}`, `t=${TOKEN}%C0%AF`),
    ];
    for (const members of bad) {
      expect({ members, parsed: parsePairingUrl(url(members)) }).toEqual({
        members,
        parsed: { ok: false, reason: "malformed" },
      });
    }
  });

  it("reads every code that efrd can write", () => {
    const octet = fc.integer({ min: 0, max: 255 });
    const code = fc.record({
      daemonId: fc.uuid().map((id) => id.toLowerCase()),
      host: fc
        .tuple(fc.integer({ min: 64, max: 127 }), octet, octet)
        .map(([b, c, d]) => `100.${b}.${c}.${d}`),
      port: fc.integer({ min: 1, max: 65_535 }),
      fqdn: fc.option(
        fc
          .array(fc.stringMatching(/^[a-z0-9](?:[a-z0-9-]{0,10}[a-z0-9])?$/), {
            minLength: 2,
            maxLength: 4,
          })
          .map((labels) => labels.join(".")),
        { nil: undefined },
      ),
      token: fc.stringMatching(/^[A-Za-z0-9_-]{22,64}$/),
    });
    fc.assert(
      fc.property(code, (input) => {
        const base = {
          daemonId: input.daemonId,
          host: input.host,
          port: input.port,
          token: input.token,
        };
        const expected: PairingCode =
          input.fqdn === undefined ? base : { ...base, fqdn: input.fqdn };
        expect(parsePairingUrl(pairingUrl(expected))).toEqual({ ok: true, code: expected });
      }),
    );
  });
});

describe("percentDecode", () => {
  it("decodes the escapes of the efr-protocol vector", () => {
    expect(percentDecode("a%20b%26c%3Dd%2F%C3%A9")).toBe("a b&c=d/\u00e9");
    expect(percentDecode("%f0%9f%99%82")).toBe("\u{1f642}");
  });

  it("refuses a byte that efrd escapes, a bad escape and bytes that are not UTF-8", () => {
    for (const text of [
      "a b",
      "a+b",
      "a/b",
      "%",
      "%2",
      "%zz",
      "%C3",
      "%C0%AF",
      "%ED%A0%80",
      "%F4%90%80%80",
    ]) {
      expect({ text, decoded: percentDecode(text) }).toEqual({ text, decoded: undefined });
    }
  });
});

describe("isTailnetIpv4", () => {
  it("takes only 100.64.0.0/10", () => {
    expect(isTailnetIpv4("100.64.0.0")).toBe(true);
    expect(isTailnetIpv4("100.127.255.255")).toBe(true);
    for (const text of [
      "100.63.255.255",
      "100.128.0.0",
      "10.0.0.1",
      "0.0.0.0",
      "100.64.0",
      "100.64.0.256",
      "::1",
    ]) {
      expect({ text, tailnet: isTailnetIpv4(text) }).toEqual({ text, tailnet: false });
    }
  });
});
