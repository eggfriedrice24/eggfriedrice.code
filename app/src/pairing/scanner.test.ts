import { describe, expect, it } from "vitest";

import { cancellation } from "./pair";
import { scanPairingCode } from "./scanner";
import type { ScannerApi } from "./scanner";
import { settle } from "./testing";

const CODE =
  "efr://pair?v=1&d=019a9b1c-3d00-7a10-8b20-000000000007&h=100.64.0.1&p=7878&t=q83vASNFZ4mrze8BI0VniQ";

/** The system scanner in memory: the test shows it codes by hand. */
class FakeScanner implements ScannerApi {
  available = true;
  granted = true;
  launched = 0;
  dismissed = 0;
  failLaunch = false;
  private listeners = new Set<(data: string) => void>();

  requestPermission(): Promise<boolean> {
    return Promise.resolve(this.granted);
  }

  launch(): Promise<void> {
    this.launched++;
    return this.failLaunch ? Promise.reject(new Error("no scanner")) : Promise.resolve();
  }

  dismiss(): Promise<void> {
    this.dismissed++;
    return Promise.resolve();
  }

  onScanned(listener: (data: string) => void): { remove(): void } {
    this.listeners.add(listener);
    return { remove: () => this.listeners.delete(listener) };
  }

  get listening(): number {
    return this.listeners.size;
  }

  show(data: string): void {
    for (const listener of this.listeners) {
      listener(data);
    }
  }
}

describe("scanPairingCode", () => {
  it("skips other codes and ends at the first pairing code", async () => {
    const scanner = new FakeScanner();
    const scan = scanPairingCode(scanner);
    await settle();
    expect(scanner.launched).toBe(1);
    scanner.show("https://example.com/menu");
    scanner.show(CODE);
    scanner.show(`${CODE}&late=1`);
    expect(await scan).toEqual({ ok: true, text: CODE });
    expect(scanner.dismissed).toBe(1);
    expect(scanner.listening).toBe(0);
  });

  it("ends a scan that the screen cancels", async () => {
    const scanner = new FakeScanner();
    const { signal, cancel } = cancellation();
    const scan = scanPairingCode(scanner, signal);
    await settle();
    cancel();
    expect(await scan).toEqual({ ok: false, reason: "cancelled" });
    expect(scanner.dismissed).toBe(1);
    expect(scanner.listening).toBe(0);
  });

  it("does not start when the signal is already set", async () => {
    const scanner = new FakeScanner();
    const { signal, cancel } = cancellation();
    cancel();
    expect(await scanPairingCode(scanner, signal)).toEqual({ ok: false, reason: "cancelled" });
    expect(scanner.launched).toBe(0);
  });

  it("names a phone without a scanner and a camera that the user refused", async () => {
    const none = new FakeScanner();
    none.available = false;
    expect(await scanPairingCode(none)).toEqual({ ok: false, reason: "unavailable" });

    const refused = new FakeScanner();
    refused.granted = false;
    expect(await scanPairingCode(refused)).toEqual({ ok: false, reason: "permission" });
    expect(refused.launched).toBe(0);

    const broken = new FakeScanner();
    broken.failLaunch = true;
    expect(await scanPairingCode(broken)).toEqual({ ok: false, reason: "unavailable" });
    expect(broken.listening).toBe(0);
  });
});
