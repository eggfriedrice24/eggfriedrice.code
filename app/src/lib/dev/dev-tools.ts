// Whether this build has the dev tools (plan section 0): the gallery of the design
// boards, the demo machines, the render counter and the decode bench.
//
// A dev build has them. A preview build has them too: its EAS profile sets
// EXPO_PUBLIC_EFR_DEV_TOOLS to 1, and Expo writes that value into the bundle when it
// builds it. A build without the variable, such as a later store build, has none of them.
// React Native defines `process.env`, so the read is safe when the value is absent.

/** True when this build has the dev tools. */
export const DEV_TOOLS: boolean =
  (typeof __DEV__ === "boolean" && __DEV__) || process.env["EXPO_PUBLIC_EFR_DEV_TOOLS"] === "1";
