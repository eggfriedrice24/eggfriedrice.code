import { useRouter } from "expo-router";
import { useCallback } from "react";

import { PairingScreen } from "@/ui/pairing_screen";
import { useDemo } from "@/ui/session_context";

// The pairing screen, also after "Pair again".
export default function Pair() {
  const demo = useDemo();
  const router = useRouter();
  const openDemo = useCallback(() => {
    demo?.();
    router.replace("/");
  }, [demo, router]);
  return <PairingScreen {...(demo === undefined ? {} : { onDemo: openDemo })} />;
}
