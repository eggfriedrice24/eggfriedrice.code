// The actions of the dev gallery: pair and forget the demo machines through the runtime
// of the app, and open a board (model/boards.ts). A screen board sets its stage on the
// demo server first, then opens the real route; the history helper of the app opens a
// single conversation, so the stack rules hold as on a real tap.

import { useSelector } from "@tanstack/react-store";
import { useRouter } from "expo-router";

import { conversationHref } from "@/features/navigation/model/routes";
import { useRuntime, usePreferences } from "@/features/navigation/use-app-services";
import { useOpenConversation } from "@/features/navigation/use-open-conversation";
import { RENDER_BOARD, type GalleryBoard } from "./model/boards";
import { DEMO_MACHINES, demoMachine, demoPairingCode, isDemoMachine } from "./model/demo-machines";
import type { DemoServer } from "./model/demo-server";
import { DEMO_DEVICE_NAME } from "./model/demo-services";
import { STAGES } from "./model/demo-stages";

/** The demo machines in the runtime of the app. */
export interface DemoMachines {
  /** How many demo machines the phone has paired. */
  readonly paired: number;
  readonly total: number;
  /** Pairs every demo machine that is not paired, through the real pairing flow. */
  readonly pairAll: () => Promise<void>;
  /** Forgets every demo machine: its record, its key and its local data. */
  readonly forgetAll: () => Promise<void>;
}

/** The demo machines of the app's runtime. */
export function useDemoMachines(): DemoMachines {
  const runtime = useRuntime();
  const paired = useSelector(
    runtime.registry,
    (state) => state.machines.filter((machine) => isDemoMachine(machine.id)).length,
  );
  const pairAll = async (): Promise<void> => {
    await runtime.load();
    const known = new Set(runtime.registry.get().machines.map((machine) => machine.id));
    const missing = DEMO_MACHINES.filter((machine) => !known.has(machine.daemonId));
    await Promise.all(
      missing.map((machine) => runtime.pair(demoPairingCode(machine), { name: DEMO_DEVICE_NAME })),
    );
  };
  const forgetAll = async (): Promise<void> => {
    const demo = runtime.registry
      .get()
      .machines.filter((machine) => isDemoMachine(machine.id))
      .map((machine) => machine.id);
    await Promise.all(demo.map((id) => runtime.forget(id)));
  };
  return { paired, total: DEMO_MACHINES.length, pairAll, forgetAll };
}

/** Opens a board of the gallery. It rejects when the demo machines cannot pair. */
export type VisitBoard = (board: GalleryBoard) => Promise<void>;

/** The function that opens a board, with the demo machines on `server`. */
export function useVisitBoard(server: DemoServer): VisitBoard {
  const router = useRouter();
  const runtime = useRuntime();
  const preferences = usePreferences();
  const open = useOpenConversation();
  const machines = useDemoMachines();
  return async (board) => {
    const visit = board.visit;
    if (visit.kind === "board") {
      // Each visit gets a sandbox of its own (native/sandboxes.ts).
      router.push({ pathname: "/board", params: { name: visit.board, visit: String(Date.now()) } });
      return;
    }
    await machines.pairAll();
    const refs = server.stage(STAGES[visit.stage]);
    // A machine that the stage before took offline waits on its backoff ladder; it
    // connects now, so the board does not start with an old offline line.
    for (const machine of DEMO_MACHINES) {
      const view = runtime.machine(machine.daemonId)?.connection.get().view.kind;
      if (server.mode(machine.key) === "online" && view !== undefined && view !== "online") {
        runtime.retry(machine.daemonId);
      }
    }
    switch (visit.kind) {
      case "root":
        preferences.setLastMachine(demoMachine(visit.machine).daemonId);
        router.dismissAll();
        return;
      case "route":
        router.push(visit.href);
        return;
      default: {
        const targets = visit.open.flatMap((key) => {
          const ref = refs.get(key);
          return ref === undefined ? [] : [ref];
        });
        const [first] = targets;
        if (targets.length === 1 && first !== undefined) {
          open(first, visit.focus === undefined ? {} : { focus: visit.focus });
          return;
        }
        // Several conversations in a row (the back history menu): plain pushes, so each
        // one lands on the stack in order.
        for (const ref of targets) {
          router.push(conversationHref(ref));
        }
      }
    }
  };
}

/** Opens a conversation that streams at the pace of efrd, with the render counter over it. */
export function useOpenRenderCounter(server: DemoServer): () => Promise<void> {
  const router = useRouter();
  const machines = useDemoMachines();
  return async () => {
    await machines.pairAll();
    const ref = server.stage(STAGES.render).get("fast");
    if (ref !== undefined) {
      router.push({
        pathname: "/board",
        params: { name: RENDER_BOARD, machine: ref.machine, id: ref.id },
      });
    }
  };
}
