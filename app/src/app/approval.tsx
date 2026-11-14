import { useLocalSearchParams } from "expo-router";

import { ApprovalSheetScreen } from "@/features/conversation/screens/approval-sheet-screen";
import { approvalParamsSchema } from "@/features/navigation/model/route-schemas";

export { ErrorBoundary } from "expo-router";

// The approval sheet of one question: `?machine=<id>&id=<conversation>&call=<call id>`.
export default function Approval() {
  const params = approvalParamsSchema.safeParse(useLocalSearchParams());
  return <ApprovalSheetScreen target={params.success ? params.data : undefined} />;
}
