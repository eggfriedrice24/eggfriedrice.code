import { beforeEach, describe, expect, it, jest } from "@jest/globals";
import {
  AuthenticationType,
  getEnrolledLevelAsync,
  SecurityLevel,
  supportedAuthenticationTypesAsync,
} from "expo-local-authentication";

import { ownerFacts } from "./owner-check";

jest.mock("expo-local-authentication", () => ({
  AuthenticationType: { FINGERPRINT: 1, FACIAL_RECOGNITION: 2, IRIS: 3 },
  SecurityLevel: { NONE: 0, SECRET: 1, BIOMETRIC_WEAK: 2, BIOMETRIC_STRONG: 3 },
  getEnrolledLevelAsync: jest.fn(),
  supportedAuthenticationTypesAsync: jest.fn(),
  authenticateAsync: jest.fn(),
}));

function iPhone(level: SecurityLevel, types: AuthenticationType[]): void {
  jest.mocked(getEnrolledLevelAsync).mockResolvedValue(level);
  jest.mocked(supportedAuthenticationTypesAsync).mockResolvedValue(types);
}

beforeEach(() => {
  jest.mocked(getEnrolledLevelAsync).mockReset();
  jest.mocked(supportedAuthenticationTypesAsync).mockReset();
});

describe("ownerFacts", () => {
  it("names Face ID when it is set up", async () => {
    iPhone(SecurityLevel.BIOMETRIC_STRONG, [AuthenticationType.FACIAL_RECOGNITION]);
    await expect(ownerFacts()).resolves.toEqual({ passcode: true, method: "Face ID" });
  });

  it("names the passcode when Face ID is not set up", async () => {
    iPhone(SecurityLevel.SECRET, [AuthenticationType.FACIAL_RECOGNITION]);
    await expect(ownerFacts()).resolves.toEqual({ passcode: true, method: "the passcode" });
  });

  it("keeps the name of the hardware on an iPhone with no passcode (Settings r9 5, 6)", async () => {
    iPhone(SecurityLevel.NONE, [AuthenticationType.FACIAL_RECOGNITION]);
    await expect(ownerFacts()).resolves.toEqual({ passcode: false, method: "Face ID" });
    iPhone(SecurityLevel.NONE, []);
    await expect(ownerFacts()).resolves.toEqual({ passcode: false, method: "the passcode" });
  });
});
