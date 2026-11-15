// The search field of a sheet (design-spec 3.15): 40 pt high, radius 20, a 1 pt
// border_subtle line instead of the system fill, the magnifier at 17 pt in text_subtle,
// and the text at 15 pt in text_primary with the placeholder in text_subtle. It uses the
// system font, as UISearchTextField does. The All conversations sheet and the model
// sheet (S-ModelSheet r7) use it.

import { TextInput, View } from "react-native";

import { roles } from "@/theme/roles";
import { cn } from "./cn";
import { SymbolIcon } from "./symbol";

export interface SearchFieldProps {
  readonly value: string;
  readonly onChangeText: (text: string) => void;
  /** The placeholder; "Search" when absent. */
  readonly placeholder?: string;
  /** The words for VoiceOver, such as "Search conversations". */
  readonly accessibilityLabel?: string;
  /** Called with true when the field gets the focus and with false when it loses it. */
  readonly onFocusChange?: ((focused: boolean) => void) | undefined;
  /** The classes of the frame; the conversations sheet keeps 16 pt from the sides. */
  readonly className?: string;
  readonly testID?: string;
}

/** The search field of a sheet. */
export function SearchField(props: SearchFieldProps) {
  const onFocusChange = props.onFocusChange;
  return (
    <View
      className={cn(
        "h-40 flex-row items-center gap-8 rounded-field border border-border-subtle px-14",
        props.className ?? "mx-inset",
      )}
    >
      <SymbolIcon name="magnifyingglass" size={17} tone="text_subtle" />
      <TextInput
        value={props.value}
        onChangeText={props.onChangeText}
        onFocus={() => onFocusChange?.(true)}
        onBlur={() => onFocusChange?.(false)}
        placeholder={props.placeholder ?? "Search"}
        placeholderTextColor={roles.text_subtle}
        selectionColor={roles.accent_primary}
        accessibilityLabel={props.accessibilityLabel ?? "Search conversations"}
        accessibilityRole="search"
        autoCapitalize="none"
        autoCorrect={false}
        clearButtonMode="while-editing"
        enterKeyHint="search"
        returnKeyType="search"
        keyboardAppearance="dark"
        className="h-40 flex-1 text-row text-text-primary"
        testID={props.testID ?? "search-field"}
      />
    </View>
  );
}
