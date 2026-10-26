// The types of the jest matchers of the component and router tests (*.test.tsx).
//
// expo-router/testing-library adds its matchers (toHavePathname) to the "expect" module.
// React Native Testing Library adds its matchers (toBeOnTheScreen) to "@jest/expect",
// which only re-exports the interface of "expect". TypeScript 7 keeps both sets only when
// the augmentation of expo-router comes first in the program. Without this file, the
// first test of src/ that imports the testing library sets the order, and then
// `toBeOnTheScreen` is missing in every test. tsc reads the files of app/ before the
// files of its folders, so this file sets the order for all of them.

/// <reference types="expo-router/testing-library" />
/// <reference types="@testing-library/react-native" />
