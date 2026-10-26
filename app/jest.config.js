// Jest runs only the component and router tests (*.test.tsx). Vitest runs the rest.
//
// The TanStack packages ship only ES modules, so jest must transform them, as it does the
// packages of React Native and Expo. The other patterns are the ones of the preset.
const preset = require("jest-expo/ios/jest-preset");

/** @type {import('jest').Config} */
module.exports = {
  preset: "jest-expo/ios",
  testMatch: ["<rootDir>/src/**/*.test.tsx", "<rootDir>/test/**/*.test.tsx"],
  transformIgnorePatterns: preset.transformIgnorePatterns.map((pattern) =>
    pattern.replace("|standard-navigation)", "|standard-navigation|@tanstack)"),
  ),
};
