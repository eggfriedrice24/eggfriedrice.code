import { useLocalSearchParams } from "expo-router";

import { accountParamsSchema } from "@/features/navigation/model/route-schemas";
import { AccountScreen } from "@/features/settings/screens/account-screen";

export { ErrorBoundary } from "expo-router";

// The limits of one account: `?machine=<id>&provider=<provider id>`.
export default function Account() {
  const params = accountParamsSchema.safeParse(useLocalSearchParams());
  return <AccountScreen target={params.success ? params.data : undefined} />;
}
