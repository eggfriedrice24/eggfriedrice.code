// The adapter of expo-notifications, with fakes of its native functions and of the app
// config. No test reaches Expo or Apple.

import { beforeEach, describe, expect, it, jest } from "@jest/globals";

import { nativePush } from "./push";

const PROJECT_ID = "419d2f79-b2ca-4c12-9a28-b97fc000bbe5";
const DEFAULT_ACTION = "expo.modules.notifications.actions.DEFAULT";
const DESKTOP = "019a9b1c-3d00-7a10-8b20-0000000000d1";
const CONVERSATION = "019a9b1c-3d00-7a10-8b20-000000001001";

const mockParts = { attach: false, voice: false, push: true };
// A getter: the adapter reads the parts when it runs, after this file set them.
jest.mock("@/lib/native/native-parts", () => ({
  get NATIVE_PARTS() {
    return mockParts;
  },
}));
jest.mock("expo-constants", () => ({
  __esModule: true,
  default: {
    expoConfig: { extra: { eas: { projectId: "419d2f79-b2ca-4c12-9a28-b97fc000bbe5" } } },
  },
}));

type TokenListener = (token: { type: string; data: string }) => void;
const mockTokenListeners: TokenListener[] = [];
const mockNotifications = {
  DEFAULT_ACTION_IDENTIFIER: "expo.modules.notifications.actions.DEFAULT",
  setNotificationHandler: jest.fn(
    (_handler: { readonly handleNotification: () => Promise<unknown> }) => undefined,
  ),
  getDevicePushTokenAsync: jest.fn(() => Promise.resolve({ type: "ios", data: "apns-1" })),
  getExpoPushTokenAsync: jest.fn((_options: unknown) =>
    Promise.resolve({ type: "expo", data: "ExponentPushToken[test-1]" }),
  ),
  addPushTokenListener: jest.fn((listener: TokenListener) => {
    mockTokenListeners.push(listener);
    return { remove: () => undefined };
  }),
  getLastNotificationResponse: jest.fn((): unknown => null),
  getPresentedNotificationsAsync: jest.fn((): Promise<unknown[]> => Promise.resolve([])),
  dismissNotificationAsync: jest.fn((_id: string) => Promise.resolve()),
};
jest.mock("expo-notifications", () => mockNotifications);

/** A delivered notice of expo-notifications. */
function notice(id: string, data: unknown) {
  return { request: { identifier: id, content: { data } } };
}

/** The response of expo-notifications to a tap with `action` on a notice. */
function response(id: string, data: unknown, action = DEFAULT_ACTION) {
  return { actionIdentifier: action, notification: notice(id, data) };
}

function adapter() {
  const push = nativePush();
  if (push === null) {
    throw new Error("the adapter needs expo-notifications");
  }
  return push;
}

beforeEach(() => {
  mockParts.push = true;
  mockTokenListeners.length = 0;
  jest.clearAllMocks();
});

describe("nativePush", () => {
  it("is null in a build without expo-notifications", () => {
    mockParts.push = false;
    expect(nativePush()).toBeNull();
    expect(mockNotifications.setNotificationHandler).not.toHaveBeenCalled();
  });

  it("shows no banner, no list entry and plays no sound while the app is in front", async () => {
    adapter();
    const [handler] = mockNotifications.setNotificationHandler.mock.calls[0] ?? [];
    expect(await handler?.handleNotification()).toEqual({
      shouldShowBanner: false,
      shouldShowList: false,
      shouldPlaySound: false,
      shouldSetBadge: false,
    });
  });

  it("gets the Expo token with the project id of the app config", async () => {
    const push = adapter();
    expect(await push.expoToken()).toBe("ExponentPushToken[test-1]");
    expect(mockNotifications.getExpoPushTokenAsync).toHaveBeenCalledWith({
      projectId: PROJECT_ID,
      devicePushToken: { type: "ios", data: "apns-1" },
    });
  });

  it("tells only of an APNs token other than the one of the last Expo token", async () => {
    const push = adapter();
    await push.expoToken();
    const changed = jest.fn();
    push.followTokens(changed);
    mockTokenListeners.forEach((listener) => listener({ type: "ios", data: "apns-1" }));
    expect(changed).not.toHaveBeenCalled();
    mockTokenListeners.forEach((listener) => listener({ type: "ios", data: "apns-2" }));
    expect(changed).toHaveBeenCalledTimes(1);
  });

  it("gives the tap that started the app, and no tap on an action button", () => {
    mockNotifications.getLastNotificationResponse.mockReturnValueOnce(
      response("n1", { d: DESKTOP, c: CONVERSATION }),
    );
    expect(adapter().launch()).toEqual({ id: "n1", data: { d: DESKTOP, c: CONVERSATION } });
    mockNotifications.getLastNotificationResponse.mockReturnValueOnce(
      response("n2", { d: DESKTOP, c: CONVERSATION }, "reply"),
    );
    expect(adapter().launch()).toBeUndefined();
  });

  it("removes the delivered notices of one conversation only", async () => {
    mockNotifications.getPresentedNotificationsAsync.mockResolvedValueOnce([
      notice("n1", { d: DESKTOP, c: CONVERSATION }),
      notice("n2", { d: DESKTOP, c: "019a9b1c-3d00-7a10-8b20-000000001002" }),
    ]);
    await adapter().dismiss(CONVERSATION);
    expect(mockNotifications.dismissNotificationAsync.mock.calls).toEqual([["n1"]]);
  });
});
