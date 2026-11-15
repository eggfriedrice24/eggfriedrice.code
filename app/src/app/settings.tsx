import { useLocalSearchParams } from "expo-router";

import { settingsParamsSchema } from "@/features/navigation/model/route-schemas";
import { SettingsScreen } from "@/features/settings/screens/settings-screen";

export { ErrorBoundary } from "expo-router";

// Settings, with `focus=unlock` from the item "Turn on Face ID unlock" of the mode menu.
export default function Settings() {
  const params = settingsParamsSchema.safeParse(useLocalSearchParams());
  return <SettingsScreen focus={params.success ? params.data.focus : undefined} />;
}
