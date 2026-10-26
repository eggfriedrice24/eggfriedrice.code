// A placeholder screen for the waves of the UI rebuild: it shows the name of the screen
// in mono text and a list of links, so the app stays navigable while the real screens
// are not built. Each real screen replaces its stub; delete this file with the last stub.
//
// NOTE: The stubs use `style` objects, not uniwind classes, because uniwind and the theme
// tokens come in other packages of the same wave. The colours are palette roles.

import type { ReactNode } from "react";
import { Pressable, ScrollView, Text, View } from "react-native";

import { MONO_FONT } from "@/theme/markdown";
import { semantic } from "@/theme/palette";

export interface ScreenStubProps {
  /** The name of the screen, such as "settings". */
  readonly name: string;
  /** More lines under the name, such as the route params. */
  readonly details?: readonly string[];
  /** The links to other screens. */
  readonly children?: ReactNode;
}

const ROOT = {
  flexGrow: 1,
  justifyContent: "center",
  gap: 12,
  paddingHorizontal: 16,
  paddingVertical: 120,
  backgroundColor: semantic.surface_1,
} as const;

const NAME = { fontFamily: MONO_FONT, fontSize: 13, color: semantic.text_bright } as const;

const DETAIL = { fontFamily: MONO_FONT, fontSize: 11, color: semantic.text_muted } as const;

const LINKS = { gap: 4, paddingTop: 12 } as const;

const LINK = { minHeight: 44, justifyContent: "center" } as const;

const LINK_TEXT = { fontFamily: MONO_FONT, fontSize: 13, color: semantic.accent_ui } as const;

/** A placeholder screen with the name `name`. */
export function ScreenStub(props: ScreenStubProps) {
  return (
    <ScrollView contentContainerStyle={ROOT} contentInsetAdjustmentBehavior="automatic">
      <Text style={NAME} accessibilityRole="header">
        {props.name}
      </Text>
      {props.details?.map((line) => (
        <Text key={line} style={DETAIL}>
          {line}
        </Text>
      ))}
      <View style={LINKS}>{props.children}</View>
    </ScrollView>
  );
}

export interface StubLinkProps {
  readonly label: string;
  readonly onPress: () => void;
}

/** One link of a placeholder screen. */
export function StubLink(props: StubLinkProps) {
  return (
    <Pressable style={LINK} accessibilityRole="link" onPress={props.onPress}>
      <Text style={LINK_TEXT}>{props.label}</Text>
    </Pressable>
  );
}
