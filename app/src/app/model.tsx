import { useLocalSearchParams } from "expo-router";

import { ModelSheetScreen } from "@/features/conversation/screens/model-sheet-screen";
import { modelSheetParamsSchema } from "@/features/navigation/model/route-schemas";

export { ErrorBoundary } from "expo-router";

// The model sheet of one composer: `?machine=<id>&id=<conversation or local id>`, with
// `root=1` for the blank root, and `model` and `provider` for the row to select first.
export default function Model() {
  const params = modelSheetParamsSchema.safeParse(useLocalSearchParams());
  return <ModelSheetScreen target={params.success ? params.data : undefined} />;
}
