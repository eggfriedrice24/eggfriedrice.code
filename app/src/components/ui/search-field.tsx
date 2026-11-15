// The search field of the All conversations sheet (design-spec 3.15): 40 pt high, radius
// 20, a 1 pt border_subtle line instead of the system fill, the magnifier at 17 pt in
// text_subtle, and the text at 15 pt in text_primary with the placeholder in
// text_subtle. It uses the system font, as UISearchTextField does.
//
// NOTE: the hard rule of app/AGENTS.md asks for the native search bar. The model sheet
// moved to it (Stack.SearchBar); the All conversations sheet waits for the user's
// decision, with a board, before it moves too. Do not add a new user of this field.

import { TextInput, View } from "react-native";

import { roles } from "@/theme/roles";
import { SymbolIcon } from "./symbol";

export interface SearchFieldProps {
  readonly value: string;
  readonly onChangeText: (text: string) => void;
}

/** The search field of the All conversations sheet. */
export function SearchField(props: SearchFieldProps) {
  return (
    <View className="mx-inset h-40 flex-row items-center gap-8 rounded-field border border-border-subtle px-14">
      <SymbolIcon name="magnifyingglass" size={17} tone="text_subtle" />
      <TextInput
        value={props.value}
        onChangeText={props.onChangeText}
        placeholder="Search"
        placeholderTextColor={roles.text_subtle}
        selectionColor={roles.accent_primary}
        accessibilityLabel="Search conversations"
        accessibilityRole="search"
        autoCapitalize="none"
        autoCorrect={false}
        clearButtonMode="while-editing"
        enterKeyHint="search"
        returnKeyType="search"
        keyboardAppearance="dark"
        className="h-40 flex-1 text-row text-text-primary"
        testID="search-field"
      />
    </View>
  );
}
