// The actions of the header buttons (design section 2, plan section 2.5):
//
// - Settings pushes Settings.
// - `+` pops to the blank root. On the blank root itself it clears the draft of the root
//   (design item 8.17), so the root is blank again.
// - "Pair a new machine" presents Pairing.

import { useRouter } from "expo-router";
import {
  useNavigation,
  type NavigationProp,
  type ParamListBase,
} from "expo-router/react-navigation";

import { BLANK_ROOT, draftKey } from "@/features/conversation/model/drafts";
import type { MachineId } from "@/features/machines/model/machine-ref";
import { SETTINGS_HREF, pairHref } from "./model/routes";
import { useDrafts } from "./use-app-services";

/** The actions of the header. */
export interface HeaderActions {
  readonly openSettings: () => void;
  /** `+`. */
  readonly newConversation: () => void;
  /** Presents Pairing, or the re-pair of `machine`. */
  readonly pairMachine: (machine?: MachineId) => void;
}

/** The actions of the header. `rootMachine` is the machine of the blank root, if any. */
export function useHeaderActions(rootMachine: MachineId | undefined): HeaderActions {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const router = useRouter();
  const drafts = useDrafts();
  return {
    openSettings: () => router.push(SETTINGS_HREF),
    newConversation: () => {
      const routes = navigation.getState()?.routes.length ?? 1;
      if (routes > 1) {
        router.dismissAll();
      } else if (rootMachine !== undefined) {
        drafts.clear(draftKey(rootMachine, BLANK_ROOT));
      }
    },
    pairMachine: (machine) => router.push(pairHref(machine)),
  };
}
