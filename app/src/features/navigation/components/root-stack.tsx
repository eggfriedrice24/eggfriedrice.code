// The root stack of the app (plan section 2.5): the routes, their presentation and the
// header of each screen.
//
// The header of a screen is set from a frame around the screen (`screenLayout`), not from
// the screen itself. So the screens of the other features hold only their content, and
// the header follows the route and its params. Each screen waits under Suspense until the
// runtime read the paired machines; until then it shows the background colour.

import { DarkTheme, Stack, ThemeProvider } from "expo-router";
import { Suspense, createContext, use, type ReactElement, type ReactNode } from "react";
import { View } from "react-native";

import { roles } from "@/theme/roles";
import { ROUTE_NAMES, conversationOfParams } from "../model/routes";
import {
  CONVERSATIONS_OPTIONS,
  CONVERSATION_OPTIONS,
  PAIR_OPTIONS,
  ROOT_OPTIONS,
  SETTINGS_OPTIONS,
  screenOptions,
} from "../model/screen-options";
import { useAppServices } from "../use-app-services";
import { ConversationHeader, RootHeader, type MachineMenuSlot } from "./screen-headers";

export interface RootStackProps {
  /** The machine menu of the blank root, from the machines feature. */
  readonly machineMenu: MachineMenuSlot;
}

/** The theme of the navigation: the dark one, with the roles of the design system. */
const THEME = {
  ...DarkTheme,
  colors: {
    ...DarkTheme.colors,
    primary: roles.accent_primary,
    background: roles.surface_1,
    card: roles.surface_1,
    text: roles.text_primary,
    border: roles.border_subtle,
    notification: roles.accent_primary,
  },
};

/** The machine menu that the root layout gives to the header of the blank root. */
const MachineMenuContext = createContext<MachineMenuSlot>(() => null);

/** The root stack. */
export function RootStack(props: RootStackProps) {
  return (
    <ThemeProvider value={THEME}>
      <MachineMenuContext value={props.machineMenu}>
        <Stack
          screenOptions={({ route }) => screenOptions(route.params)}
          screenLayout={screenLayout}
        >
          <Stack.Screen name={ROUTE_NAMES.root} options={ROOT_OPTIONS} />
          <Stack.Screen name={ROUTE_NAMES.conversation} options={CONVERSATION_OPTIONS} />
          <Stack.Screen name={ROUTE_NAMES.settings} options={SETTINGS_OPTIONS} />
          <Stack.Screen name={ROUTE_NAMES.conversations} options={CONVERSATIONS_OPTIONS} />
          <Stack.Screen name={ROUTE_NAMES.pair} options={PAIR_OPTIONS} />
        </Stack>
      </MachineMenuContext>
    </ThemeProvider>
  );
}

/** A route of the stack, as the frame reads it. */
interface FrameRoute {
  readonly name: string;
  readonly params?: object | undefined;
}

/**
 * The header element of each route object. The navigator calls `screenLayout` on each of
 * its renders, and a header sets the options of its route, which renders the navigator
 * again. The same element for the same route lets React skip the header then, so the
 * two never render each other without end. A route object changes with its params.
 */
const HEADERS = new WeakMap<FrameRoute, ReactElement>();

function headerOf(route: FrameRoute): ReactElement {
  let header = HEADERS.get(route);
  if (header === undefined) {
    header = <ScreenHeader name={route.name} params={route.params} />;
    HEADERS.set(route, header);
  }
  return header;
}

/** The frame of each screen of the stack. */
function screenLayout(args: { readonly route: FrameRoute; readonly children: ReactNode }) {
  return <ScreenFrame header={headerOf(args.route)}>{args.children}</ScreenFrame>;
}

interface ScreenFrameProps {
  readonly header: ReactElement;
  readonly children: ReactNode;
}

/** One screen with its header. */
function ScreenFrame(props: ScreenFrameProps) {
  return (
    <View className="flex-1 bg-surface-1">
      <Suspense fallback={null}>
        <Loaded>
          {props.children}
          {props.header}
        </Loaded>
      </Suspense>
    </View>
  );
}

/** Renders `children` once the runtime read the paired machines. */
function Loaded(props: { readonly children: ReactNode }) {
  use(useAppServices().loaded);
  return props.children;
}

interface ScreenHeaderProps {
  readonly name: string;
  readonly params: object | undefined;
}

function ScreenHeader(props: ScreenHeaderProps) {
  const machineMenu = use(MachineMenuContext);
  if (props.name === ROUTE_NAMES.root) {
    return <RootHeader machineMenu={machineMenu} />;
  }
  if (props.name === ROUTE_NAMES.conversation) {
    const conversation = conversationOfParams(props.params);
    return conversation === undefined ? null : <ConversationHeader conversation={conversation} />;
  }
  return null;
}
