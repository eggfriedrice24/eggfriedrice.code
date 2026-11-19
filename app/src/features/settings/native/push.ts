// The native side of push and of the taps on a notice, on expo-notifications. A build
// without the module of expo-notifications gives null: its package throws when it loads,
// so this file loads it only when the build has it (lib/native/native-parts.ts).
//
// - While the app is in front, a notice shows no banner, goes to no list and plays no
//   sound: the transcript shows the question already.
// - The Expo push token comes with the project id of the app config
//   (`extra.eas.projectId`), the one EAS project of the three variants. The call takes
//   the APNs environment and the bundle id from the build.
// - iOS gives the app its APNs token again on each registration, also on the one of an
//   Expo token, so only a token other than the last one counts as new.
// - Only a tap on the notice itself opens its conversation.

import Constants from "expo-constants";
import { Linking } from "react-native";

import { NATIVE_PARTS } from "@/lib/native/native-parts";
import type { NoticeSource, NoticeTap } from "@/features/navigation/model/notice-taps";
import type { PushNative, PushPermission } from "../model/push";

type Notifications = typeof import("expo-notifications");
type NotificationResponse = import("expo-notifications").NotificationResponse;

/** What a notice does while the app is in front: nothing that the user sees or hears. */
const IN_FRONT = {
  shouldShowBanner: false,
  shouldShowList: false,
  shouldPlaySound: false,
  shouldSetBadge: false,
} as const;

/** The EAS project id of the app config, or undefined when the config has none. */
function projectId(): string | undefined {
  const id: unknown = Constants.expoConfig?.extra?.["eas"]?.projectId;
  return typeof id === "string" && id !== "" ? id : undefined;
}

function permissionOf(status: string): PushPermission {
  return status === "granted" || status === "denied" ? status : "undetermined";
}

/** The tap of `response`, or undefined when it was not a tap on the notice itself. */
function tapOf(
  notifications: Notifications,
  response: NotificationResponse,
): NoticeTap | undefined {
  if (response.actionIdentifier !== notifications.DEFAULT_ACTION_IDENTIFIER) {
    return undefined;
  }
  const request = response.notification.request;
  // NOTE: efrd gives each notice of a conversation the same id (its collapse id), so
  // only the id with the date names one notice: the launch tap and its copy from the
  // listener are equal, and a later notice of the conversation is not.
  const id = `${request.identifier}@${response.notification.date}`;
  return { id, data: request.content.data };
}

/** The push effects of the app, or null in a build without expo-notifications. */
export function nativePush(): (PushNative & NoticeSource) | null {
  if (!NATIVE_PARTS.push) {
    return null;
  }
  const notifications: Notifications = require("expo-notifications");
  notifications.setNotificationHandler({ handleNotification: () => Promise.resolve(IN_FRONT) });
  let deviceToken: string | undefined;
  return {
    async permission() {
      return permissionOf((await notifications.getPermissionsAsync()).status);
    },
    async requestPermission() {
      const answer = await notifications.requestPermissionsAsync({
        ios: { allowAlert: true, allowSound: true, allowBadge: false },
      });
      return permissionOf(answer.status);
    },
    async expoToken() {
      const id = projectId();
      if (id === undefined) {
        throw new Error("the app config has no EAS project id (extra.eas.projectId)");
      }
      const device = await notifications.getDevicePushTokenAsync();
      deviceToken = String(device.data);
      const expo = await notifications.getExpoPushTokenAsync({
        projectId: id,
        devicePushToken: device,
      });
      return expo.data;
    },
    followTokens(listener) {
      const subscription = notifications.addPushTokenListener((token) => {
        if (String(token.data) !== deviceToken) {
          listener();
        }
      });
      return () => subscription.remove();
    },
    async dismiss(conversation) {
      const presented = await notifications.getPresentedNotificationsAsync();
      await Promise.all(
        presented
          .filter((notice) => notice.request.content.data?.["c"] === conversation)
          .map((notice) => notifications.dismissNotificationAsync(notice.request.identifier)),
      );
    },
    openSettings() {
      void Linking.openSettings().catch(() => undefined);
    },
    launch() {
      const response = notifications.getLastNotificationResponse();
      return response === null ? undefined : tapOf(notifications, response);
    },
    follow(listener) {
      const subscription = notifications.addNotificationResponseReceivedListener((response) => {
        const tap = tapOf(notifications, response);
        if (tap !== undefined) {
          listener(tap);
        }
      });
      return () => subscription.remove();
    },
    clear() {
      notifications.clearLastNotificationResponse();
    },
  };
}
