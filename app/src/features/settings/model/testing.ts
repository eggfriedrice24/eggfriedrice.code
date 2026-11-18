// Fakes for the tests of push: the native side of expo-notifications, which a test drives
// step by step. Only test files import this module.

import type { NoticeSource, NoticeTap } from "@/features/navigation/model/notice-taps";
import type { PushNative, PushPermission } from "./push";

/** An Expo push token of the tests. */
export function expoToken(n: number): string {
  return `ExponentPushToken[test-${String(n)}]`;
}

/** The native side of push and of the taps, in memory. */
export class FakePushNative implements PushNative, NoticeSource {
  /** What iOS allows now. */
  permissionNow: PushPermission = "undetermined";
  /** What the next question of iOS answers. */
  answer: PushPermission = "granted";
  /** The token that the next request to Expo gives; undefined makes it fail. */
  nextToken: string | undefined = expoToken(1);
  questions = 0;
  tokenRequests = 0;
  settingsOpened = 0;
  readonly dismissed: string[] = [];
  launchTap: NoticeTap | undefined;
  cleared = 0;
  private readonly tokenListeners = new Set<() => void>();
  private readonly tapListeners = new Set<(tap: NoticeTap) => void>();

  permission(): Promise<PushPermission> {
    return Promise.resolve(this.permissionNow);
  }

  requestPermission(): Promise<PushPermission> {
    this.questions += 1;
    if (this.permissionNow === "undetermined") {
      this.permissionNow = this.answer;
    }
    return Promise.resolve(this.permissionNow);
  }

  expoToken(): Promise<string> {
    this.tokenRequests += 1;
    const token = this.nextToken;
    return token === undefined ? Promise.reject(new Error("no network")) : Promise.resolve(token);
  }

  followTokens(listener: () => void): () => void {
    this.tokenListeners.add(listener);
    return () => {
      this.tokenListeners.delete(listener);
    };
  }

  /** iOS gives the app a new APNs token. */
  newApnsToken(): void {
    for (const listener of this.tokenListeners) {
      listener();
    }
  }

  dismiss(conversation: string): Promise<void> {
    this.dismissed.push(conversation);
    return Promise.resolve();
  }

  openSettings(): void {
    this.settingsOpened += 1;
  }

  launch(): NoticeTap | undefined {
    return this.launchTap;
  }

  follow(listener: (tap: NoticeTap) => void): () => void {
    this.tapListeners.add(listener);
    return () => {
      this.tapListeners.delete(listener);
    };
  }

  clear(): void {
    this.cleared += 1;
    this.launchTap = undefined;
  }

  /** The user taps a notice. */
  tap(tap: NoticeTap): void {
    for (const listener of this.tapListeners) {
      listener(tap);
    }
  }
}
