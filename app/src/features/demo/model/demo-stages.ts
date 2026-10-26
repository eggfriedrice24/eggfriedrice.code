// The stages of the demo server: what the demo machines hold and how they answer for
// each board of the gallery that opens a real screen (boards.ts). Pure.

import type { DemoStage, StageConversation } from "./demo-server";

/** The conversations that a busy machine has, besides the one of the board. */
function recentOn(machine: StageConversation["machine"]): StageConversation[] {
  return [
    { key: `${machine}-notes`, machine, kind: "notes", minutesAgo: 4 },
    { key: `${machine}-disk`, machine, kind: "disk", minutesAgo: 35 },
    { key: `${machine}-rename`, machine, kind: "rename", minutesAgo: 180 },
  ];
}

/** Every stage, by name. */
export const STAGES = {
  /** Three machines online with a few conversations each. */
  machines: {
    name: "machines",
    conversations: [...recentOn("desktop"), ...recentOn("laptop"), ...recentOn("homelab")],
  },
  /** A reply that streams with a thought, tools, a code block and a queued prompt. */
  streaming: {
    name: "streaming",
    conversations: [
      { key: "streaming", machine: "desktop", kind: "streaming", minutesAgo: 1 },
      ...recentOn("desktop"),
    ],
  },
  /** A question in this conversation that waits for you. */
  approvalHere: {
    name: "approval here",
    conversations: [
      { key: "approval", machine: "desktop", kind: "approval", minutesAgo: 1 },
      ...recentOn("desktop"),
    ],
  },
  /** A question in another conversation: the title dot and the strip. */
  approvalElsewhere: {
    name: "approval elsewhere",
    conversations: [
      { key: "waiting", machine: "laptop", kind: "waiting", minutesAgo: 2 },
      ...recentOn("desktop"),
      ...recentOn("laptop"),
    ],
  },
  /** Two questions in other conversations: "2 approvals waiting". */
  approvalsElsewhere: {
    name: "approvals elsewhere",
    conversations: [
      { key: "waiting", machine: "desktop", kind: "waiting", minutesAgo: 2 },
      { key: "waiting-2", machine: "laptop", kind: "waiting", minutesAgo: 6 },
      ...recentOn("desktop"),
      ...recentOn("laptop"),
    ],
  },
  /** Many conversations, two of them waiting, for the title menu and the sheet. */
  many: {
    name: "many conversations",
    conversations: [
      { key: "waiting", machine: "desktop", kind: "waiting", minutesAgo: 3 },
      { key: "waiting-2", machine: "laptop", kind: "waiting", minutesAgo: 9 },
      ...recentOn("desktop"),
      { key: "failed", machine: "desktop", kind: "failed", minutesAgo: 60 * 26 },
      { key: "markdown", machine: "desktop", kind: "markdown", minutesAgo: 60 * 30 },
      { key: "diff", machine: "desktop", kind: "diff", minutesAgo: 60 * 24 * 3 },
      { key: "messages", machine: "desktop", kind: "messages", minutesAgo: 60 * 24 * 5 },
      { key: "tools", machine: "desktop", kind: "tools", minutesAgo: 60 * 24 * 12 },
      ...recentOn("laptop"),
      ...recentOn("homelab"),
    ],
  },
  /** The machine of the conversation goes offline after the transcript came. */
  offline: {
    name: "offline",
    conversations: [...recentOn("laptop")],
    changes: [{ after: "laptop-disk", afterMs: 2500, machine: "laptop", mode: "offline" }],
  },
  /** The machine of the conversation stops answering after the transcript came. */
  reconnecting: {
    name: "reconnecting",
    conversations: [...recentOn("homelab")],
    changes: [{ after: "homelab-notes", afterMs: 2500, machine: "homelab", mode: "silent" }],
  },
  /** The machine asks for a new pairing after the transcript came. */
  repair: {
    name: "re-pair needed",
    conversations: [...recentOn("laptop")],
    changes: [{ after: "laptop-notes", afterMs: 2500, machine: "laptop", mode: "repair" }],
  },
  /** efrd of the machine starts again after the transcript came. */
  starting: {
    name: "efrd is starting",
    conversations: [...recentOn("homelab")],
    changes: [{ after: "homelab-notes", afterMs: 2500, machine: "homelab", mode: "starting" }],
  },
  /** efrd of the machine is too old for the app. */
  tooOld: {
    name: "efrd too old",
    conversations: [...recentOn("homelab")],
    changes: [{ after: "homelab-notes", afterMs: 2500, machine: "homelab", mode: "old" }],
  },
  /** An interrupted turn and a failed turn. */
  failed: {
    name: "failed",
    conversations: [
      { key: "failed", machine: "desktop", kind: "failed", minutesAgo: 1 },
      ...recentOn("desktop"),
    ],
  },
  /** A long transcript with a reply that streams. */
  long: {
    name: "long",
    conversations: [
      { key: "long", machine: "desktop", kind: "long", minutesAgo: 1 },
      ...recentOn("desktop"),
    ],
  },
  /** A conversation whose snapshot comes late. */
  loading: {
    name: "loading",
    conversations: [
      { key: "loading", machine: "desktop", kind: "notes", minutesAgo: 1, snapshotDelayMs: 8000 },
      ...recentOn("desktop"),
    ],
  },
  /** A long reply at the pace of efrd, for the render counter. */
  render: {
    name: "render",
    conversations: [
      { key: "fast", machine: "desktop", kind: "fast", minutesAgo: 1 },
      ...recentOn("desktop"),
    ],
  },
  /** Settings: one machine connected, one offline, one connecting. */
  settings: {
    name: "settings",
    modes: { laptop: "offline", homelab: "silent" },
    conversations: [...recentOn("desktop")],
  },
} as const satisfies Record<string, DemoStage>;

/** The name of a stage. */
export type StageName = keyof typeof STAGES;
