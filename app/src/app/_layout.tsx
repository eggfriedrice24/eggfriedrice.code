import { Stack } from "expo-router";

import { semantic } from "@/theme/palette";

const screenOptions = {
  contentStyle: { backgroundColor: semantic.surface_1 },
  headerStyle: { backgroundColor: semantic.surface_1 },
  headerTintColor: semantic.text_primary,
};

export default function RootLayout() {
  return <Stack screenOptions={screenOptions} />;
}
