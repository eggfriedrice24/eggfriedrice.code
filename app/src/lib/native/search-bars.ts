// The native search bars of the screens (`Stack.SearchBar` of expo-router, a UISearchBar
// of iOS). `Keyboard.dismiss` of React Native blurs only a TextInput of React Native,
// which it tracks. It does not see a UISearchBar, so the keyboard of the bar stays. A
// screen gives its search bar a ref of `searchBarRef()`: it blurs the bar through the ref,
// and `blurSearchBars` blurs every bar that is mounted, so the lock of the app closes the
// keyboard of a bar too (`dismissInput` of app/_layout.tsx).

import type { RefObject } from "react";
import type { SearchBarCommands } from "react-native-screens";

/** The commands of each search bar that is mounted now. */
const mounted = new Set<SearchBarCommands>();

/**
 * A ref for `Stack.SearchBar`. react-native-screens sets `current` when the bar mounts
 * and sets it to null when the bar unmounts (`useImperativeHandle`), so the ref adds the
 * bar to the mounted set and takes it out again.
 */
export function searchBarRef(): RefObject<SearchBarCommands | null> {
  let commands: SearchBarCommands | null = null;
  return {
    get current() {
      return commands;
    },
    set current(next) {
      if (commands !== null) {
        mounted.delete(commands);
      }
      commands = next;
      if (next !== null) {
        mounted.add(next);
      }
    },
  };
}

/** Blurs every search bar that is mounted, so that its keyboard goes. */
export function blurSearchBars(): void {
  for (const commands of mounted) {
    commands.blur();
  }
}
