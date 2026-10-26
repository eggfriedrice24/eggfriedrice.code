// The inventory of the dev gallery (plan section 0): every board of the design canvas (30
// boards, design-spec section 1) and every undrawn state of plan 5.2 item 8.19, each with
// the way the gallery shows it.
//
// - A screen board sets a stage of the demo server and opens the real route: the blank
//   root, a conversation, Settings or the All conversations sheet. The demo machines are
//   paired through the real pairing flow first.
// - A component board opens the gallery's own route with the real component in that
//   state: the pairing boards, the component sheets, and the states that need a machine
//   setup that the runtime of the app cannot have, such as no machine at all.
//
// A menu (UIMenu) and an action sheet cannot open from code; such a board says what to
// tap. Pure.

import { PAIRING_BOARDS } from "@/features/pairing/model/pairing-boards";
import type { DemoMachineKey } from "./demo-machines";
import type { StageName } from "./demo-stages";

/** How the gallery shows a board. */
export type BoardVisit =
  /** The blank root on `machine`. */
  | { readonly kind: "root"; readonly stage: StageName; readonly machine: DemoMachineKey }
  /** The conversations of the stage with these keys, pushed in order. */
  | {
      readonly kind: "conversation";
      readonly stage: StageName;
      readonly open: readonly string[];
      readonly focus?: "approval";
    }
  /** A route of the app. */
  | {
      readonly kind: "route";
      readonly stage: StageName;
      readonly href: "/settings" | "/conversations";
    }
  /** The board route of the gallery, with component board `board`. */
  | { readonly kind: "board"; readonly board: string };

/** The section of a board in the gallery. */
export type BoardSection = "screens" | "states" | "settings" | "pairing" | "sheets" | "undrawn";

/** One board of the gallery. */
export interface GalleryBoard {
  /** The name of the board on the canvas, such as `R-Streaming`, or of an undrawn state. */
  readonly name: string;
  /** The title on the canvas, or what the undrawn state is. */
  readonly title: string;
  readonly section: BoardSection;
  /** True for a board that the canvas draws; false for an undrawn state (item 8.19). */
  readonly drawn: boolean;
  readonly visit: BoardVisit;
  /** What to do on the screen to see the board, when it needs a tap. */
  readonly hint?: string;
}

/** The titles of the sections, in the order of the gallery. */
export const SECTIONS: readonly (readonly [BoardSection, string])[] = [
  ["screens", "Root and conversation"],
  ["states", "Conversation states"],
  ["settings", "Settings"],
  ["pairing", "Pairing"],
  ["sheets", "Component sheets"],
  ["undrawn", "Undrawn states (item 8.19)"],
];

/** The prefix of the component board of a pairing board. */
export const PAIRING_PREFIX = "pairing:";

/** The board of the render counter: a conversation that streams, with its counts. */
export const RENDER_BOARD = "render";

/** The name of the component board of a pairing board. */
export function pairingBoardName(name: string): string {
  return `${PAIRING_PREFIX}${name}`;
}

const SCREENS: readonly GalleryBoard[] = [
  {
    name: "Main",
    title: "Root · blank (app launch)",
    section: "screens",
    drawn: true,
    visit: { kind: "root", stage: "machines", machine: "desktop" },
    hint: "Opens the root. Come back with a long press on the version in Settings.",
  },
  {
    name: "R-Streaming",
    title: "Conversation · streaming a reply",
    section: "screens",
    drawn: true,
    visit: { kind: "conversation", stage: "streaming", open: ["streaming"] },
  },
  {
    name: "R-Approval",
    title: "Conversation · approval here",
    section: "screens",
    drawn: true,
    visit: { kind: "conversation", stage: "approvalHere", open: ["approval"] },
  },
  {
    name: "R-Other",
    title: "Conversation · approval elsewhere",
    section: "screens",
    drawn: true,
    visit: { kind: "conversation", stage: "approvalElsewhere", open: ["desktop-notes"] },
  },
  {
    name: "R-History",
    title: "Conversation · back history menu",
    section: "screens",
    drawn: true,
    visit: {
      kind: "conversation",
      stage: "machines",
      open: ["desktop-rename", "desktop-disk", "desktop-notes"],
    },
    hint: "Long press the back button.",
  },
  {
    name: "R-NoMachine",
    title: "Root · no machine paired",
    section: "screens",
    drawn: true,
    visit: { kind: "board", board: "R-NoMachine" },
  },
  {
    name: "R-MachineMenu",
    title: "Root · machine menu",
    section: "screens",
    drawn: true,
    visit: { kind: "root", stage: "machines", machine: "desktop" },
    hint: "Tap the machine button on the left.",
  },
  {
    name: "R-TitleMenu",
    title: "Title · opens the conversations sheet (was the title menu)",
    section: "screens",
    drawn: true,
    visit: { kind: "conversation", stage: "many", open: ["desktop-notes"] },
    hint: "Tap the title: the conversations sheet opens.",
  },
  {
    name: "R-AllSheet",
    title: "All conversations sheet (2 waiting)",
    section: "screens",
    drawn: true,
    visit: { kind: "route", stage: "many", href: "/conversations" },
  },
];

const STATES: readonly GalleryBoard[] = [
  {
    name: "C-Offline",
    title: "Conversation · machine offline",
    section: "states",
    drawn: true,
    visit: { kind: "conversation", stage: "offline", open: ["laptop-disk"] },
    hint: "laptop goes offline 2.5 s after the transcript comes.",
  },
  {
    name: "C-Reconnecting",
    title: "Conversation · reconnecting",
    section: "states",
    drawn: true,
    visit: { kind: "conversation", stage: "reconnecting", open: ["homelab-notes"] },
    hint: "homelab stops answering 2.5 s after the transcript comes.",
  },
  {
    name: "C-Failed",
    title: "Conversation · failed and interrupted turns",
    section: "states",
    drawn: true,
    visit: { kind: "conversation", stage: "failed", open: ["failed"] },
  },
  {
    name: "C-Jump",
    title: "Conversation · scrolled up, jump to latest",
    section: "states",
    drawn: true,
    visit: { kind: "conversation", stage: "long", open: ["long"] },
    hint: "Scroll up while the reply streams.",
  },
];

const SETTINGS: readonly GalleryBoard[] = [
  {
    name: "Settings",
    title: "Settings (pushed)",
    section: "settings",
    drawn: true,
    visit: { kind: "route", stage: "settings", href: "/settings" },
  },
  {
    name: "Settings-Menu",
    title: "Settings · machine row menu",
    section: "settings",
    drawn: true,
    visit: { kind: "route", stage: "settings", href: "/settings" },
    hint: "Tap a machine row.",
  },
  {
    name: "Settings-Forget",
    title: "Settings · Forget confirmation",
    section: "settings",
    drawn: true,
    visit: { kind: "route", stage: "settings", href: "/settings" },
    hint: "Tap a machine row, then Forget. The next visit pairs the demo machine again.",
  },
  {
    name: "Settings-ForgetLast",
    title: "Settings · Forget the last machine",
    section: "settings",
    drawn: true,
    visit: { kind: "board", board: "Settings-ForgetLast" },
    hint: "Tap the machine row, then Forget.",
  },
];

/** The canvas titles of the pairing boards. P-Errors draws five errors on one board. */
const PAIRING_TITLES: Readonly<Record<string, string>> = {
  "P-Intro": "Pairing 1 · intro",
  "P-Scan": "Pairing 2 · system scanner",
  "P-Wait": "Pairing 3 · confirm on the machine",
  "P-Success": "Pairing 4 · success",
  "P-Errors": "Pairing 5 · errors",
};

/** The gallery board of pairing board `board` (features/pairing/model/pairing-boards.ts). */
function pairingEntry(board: (typeof PAIRING_BOARDS)[number]): GalleryBoard {
  const [canvas = "", detail] = board.name.split(" · ");
  const title = board.drawn
    ? [PAIRING_TITLES[canvas] ?? canvas, detail].filter((part) => part !== undefined).join(" · ")
    : `Pairing: ${detail ?? board.name}`;
  return {
    name: board.name,
    title,
    section: board.drawn ? "pairing" : "undrawn",
    drawn: board.drawn,
    visit: { kind: "board", board: pairingBoardName(board.name) },
  };
}

const PAIRING: readonly GalleryBoard[] = PAIRING_BOARDS.map(pairingEntry);

/** A component sheet of the canvas. */
function sheet(name: string, title: string): GalleryBoard {
  return { name, title, section: "sheets", drawn: true, visit: { kind: "board", board: name } };
}

const SHEETS: readonly GalleryBoard[] = [
  sheet("S-Composer", "Composer · anatomy and every state"),
  sheet("S-Approvals", "Approval block · every state"),
  sheet("S-Messages", "Thought, your messages, turn end"),
  sheet("S-Markdown", "Markdown"),
  sheet("S-Tools", "Tool blocks"),
  sheet("S-Diff", "Edit block expanded"),
  sheet("S-Loaders", "Loaders"),
  sheet("Icon", "App icon · the yolk (direction b)"),
];

const UNDRAWN: readonly GalleryBoard[] = [
  {
    name: "Strip · 2 approvals waiting",
    title: "The plural strip; a tap opens the sheet with the waiting section on top",
    section: "undrawn",
    drawn: false,
    visit: { kind: "conversation", stage: "approvalsElsewhere", open: ["desktop-notes"] },
  },
  {
    name: "Settings · no machine",
    title: "Settings when no machine is paired",
    section: "undrawn",
    drawn: false,
    visit: { kind: "board", board: "Settings · no machine" },
  },
  {
    name: "Conversation · re-pair needed",
    title: "The offline line and the composer when the machine asks for a new pairing",
    section: "undrawn",
    drawn: false,
    visit: { kind: "conversation", stage: "repair", open: ["laptop-notes"] },
    hint: "laptop asks for a new pairing 2.5 s after the transcript comes.",
  },
  {
    name: "Conversation · efrd is starting",
    title: "The offline line and the composer while efrd starts",
    section: "undrawn",
    drawn: false,
    visit: { kind: "conversation", stage: "starting", open: ["homelab-notes"] },
    hint: "homelab starts again 2.5 s after the transcript comes.",
  },
  {
    name: "Conversation · efrd too old",
    title: "The offline line and the composer when efrd speaks an older protocol",
    section: "undrawn",
    drawn: false,
    visit: { kind: "conversation", stage: "tooOld", open: ["homelab-notes"] },
    hint: "homelab answers with an older protocol 2.5 s after the transcript comes.",
  },
  {
    name: "Conversation · loading",
    title: "Loading a conversation: efrd sends the transcript after 8 s",
    section: "undrawn",
    drawn: false,
    visit: { kind: "conversation", stage: "loading", open: ["loading"] },
  },
  {
    name: "Root · title sheet",
    title: "The title of the blank root opens the conversations sheet",
    section: "undrawn",
    drawn: false,
    visit: { kind: "root", stage: "many", machine: "desktop" },
    hint: "Tap the title: the conversations sheet opens.",
  },
];

/** Every board of the gallery, in its order. */
export const GALLERY_BOARDS: readonly GalleryBoard[] = [
  ...SCREENS,
  ...STATES,
  ...SETTINGS,
  ...PAIRING.filter((board) => board.drawn),
  ...SHEETS,
  ...UNDRAWN,
  ...PAIRING.filter((board) => !board.drawn),
];

/** The board named `name`, or undefined. */
export function galleryBoard(name: string): GalleryBoard | undefined {
  return GALLERY_BOARDS.find((board) => board.name === name);
}

/** The boards of `section`, in order. */
export function boardsOf(section: BoardSection): GalleryBoard[] {
  return GALLERY_BOARDS.filter((board) => board.section === section);
}
