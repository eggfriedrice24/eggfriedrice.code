// Metro reads this file. uniwind compiles global.css (Tailwind 4 and the theme of
// src/theme/theme.css) and gives className to the React Native components.
const { getDefaultConfig } = require("expo/metro-config");
const { withUniwindConfig } = require("uniwind/metro");

module.exports = withUniwindConfig(getDefaultConfig(__dirname), {
  cssEntryFile: "./global.css",
  dtsFile: "./uniwind-types.d.ts",
});
