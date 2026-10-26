// Jest runs only the component and router tests (*.test.tsx). Vitest runs the rest.
/** @type {import('jest').Config} */
module.exports = {
  preset: "jest-expo/ios",
  testMatch: ["<rootDir>/src/**/*.test.tsx", "<rootDir>/test/**/*.test.tsx"],
};
