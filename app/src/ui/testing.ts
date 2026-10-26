// The replay harness of the UI tests: it runs a stream of timed items through what the
// app runs between a frame from efrd and the pixels (the envelope guard, the stream
// reducer, the details reducer, the paced text of each message row and the block
// splitter) on a 16 ms frame clock, and checks on every frame what a reader must never
// see:
//
// - the shown text of a message moves back, or stops being a prefix of its text, within
//   one generation;
// - a block that the splitter called final changes.
//
// The lag of the pacer has its own tests in src/core/pacer.test.ts.
//
// Only test files import this module.

import { decodeSubscribeItem } from "../core/envelope";
import { EMPTY_STREAM, type MessageView, reduce, type StreamState } from "../core/reducer";
import { utf16IndexAtByte, utf8Length } from "../core/utf8";
import { type BlockSplit, EMPTY_SPLIT, splitBlocks } from "./blocks";
import { EMPTY_DETAILS, reduceDetails, type TranscriptDetails } from "./details";
import { pacedArrive, pacedFrame, type PacedText, startPaced } from "./paced_text";
import type { TimedItem } from "./scenarios";

/** The frame interval of the harness. */
export const FRAME_MS = 16;

/** What a replay ends with. */
export interface ReplayResult {
  readonly stream: StreamState;
  readonly details: TranscriptDetails;
  /** The text shown on the last frame of each message, by `turnId:index`. */
  readonly shown: ReadonlyMap<string, string>;
  /** Each broken rule, in words; empty when the replay was clean. */
  readonly problems: readonly string[];
  readonly frames: number;
  /** The frames on which some message showed less than its text: the pacer at work. */
  readonly pacedFrames: number;
}

interface Row {
  message: MessageView;
  paced: PacedText;
  split: BlockSplit;
  shown: string;
}

/** Runs `items` (in the order given, each at its time) and checks every frame. */
export function replay(items: readonly TimedItem[], reduceMotion = false): ReplayResult {
  let stream = EMPTY_STREAM;
  let details = EMPTY_DETAILS;
  const rows = new Map<string, Row>();
  const problems: string[] = [];
  const end = (items[items.length - 1]?.at ?? 0) + 1_000;
  let next = 0;
  let frames = 0;
  let pacedFrames = 0;
  for (let now = 0; now <= end; now += FRAME_MS) {
    while (next < items.length && (items[next]?.at ?? 0) <= now) {
      const raw = items[next]?.item;
      next++;
      details = reduceDetails(details, raw);
      const decoded = decodeSubscribeItem(raw);
      if (!decoded.ok) {
        problems.push(`item ${next - 1} does not decode: ${decoded.reason}`);
        continue;
      }
      stream = reduce(stream, decoded.value);
      for (const [turnId, turn] of stream.turns) {
        for (const [index, message] of turn.messages) {
          const key = `${turnId}:${index}`;
          const row = rows.get(key);
          if (row === undefined) {
            if (message.buffer.text.length > 0) {
              rows.set(key, {
                message,
                paced: startPaced(input(message), now),
                split: EMPTY_SPLIT,
                shown: "",
              });
            }
          } else if (row.message !== message) {
            row.message = message;
            row.paced = pacedArrive(row.paced, input(message), now);
          }
        }
      }
    }
    frames++;
    let paced = false;
    for (const [key, row] of rows) {
      const text = row.message.buffer.text;
      const generation = row.paced.generation;
      const frame = pacedFrame(row.paced, text, now, reduceMotion);
      const changedGeneration = generation !== frame.state.generation;
      row.paced = frame.state;
      const shown = text.slice(0, frame.shown);
      if (
        !changedGeneration &&
        !shown.startsWith(row.shown) &&
        row.message.generation === generation
      ) {
        problems.push(`${key} at ${now} ms: the shown text moved back or changed`);
      }
      const split = splitBlocks(shown, changedGeneration ? EMPTY_SPLIT : row.split);
      if (!changedGeneration) {
        for (let i = 0; i < row.split.blocks.length; i++) {
          const before = row.split.blocks[i];
          const after = split.blocks[i];
          if (
            before === undefined ||
            after === undefined ||
            before.start !== after.start ||
            before.end !== after.end ||
            before.kind !== after.kind ||
            row.split.text.slice(before.start, before.end) !== shown.slice(after.start, after.end)
          ) {
            problems.push(`${key} at ${now} ms: final block ${i} changed`);
            break;
          }
        }
      }
      row.split = split;
      row.shown = shown;
      paced ||= shown.length < text.length;
    }
    if (paced) {
      pacedFrames++;
    }
  }
  const shown = new Map<string, string>();
  for (const [key, row] of rows) {
    shown.set(key, row.shown);
  }
  return { stream, details, shown, problems, frames, pacedFrames };
}

function input(message: MessageView) {
  return { text: message.buffer.text, final: message.final, generation: message.generation };
}

/** A small seeded random source (a linear congruential generator), for fault plans. */
export function seeded(seed: number): () => number {
  let state = seed >>> 0 || 1;
  return () => {
    state = (Math.imul(state, 1_664_525) + 1_013_904_223) >>> 0;
    return state / 2 ** 32;
  };
}

/** `items` without the drafts that `drop` picks. */
export function dropDrafts(
  items: readonly TimedItem[],
  drop: (index: number) => boolean,
): TimedItem[] {
  return items.filter((timed, index) => !(kindOf(timed.item) === "draft" && drop(index)));
}

/** `items` with some events sent again right after themselves, as a replay can. */
export function duplicateEvents(
  items: readonly TimedItem[],
  pick: (index: number) => boolean,
): TimedItem[] {
  const out: TimedItem[] = [];
  items.forEach((timed, index) => {
    out.push(timed);
    if (kindOf(timed.item) === "event" && pick(index)) {
      out.push(timed);
    }
  });
  return out;
}

/**
 * The stream of a phone that lost its socket after item `cut` and subscribed again at
 * item `back`, as efrd answers a subscribe without `after_seq`: a snapshot of the events
 * so far (at most `window` of them) and the live part of the running message, then the
 * items after `back`. The drafts in between are lost.
 */
export function reconnectWithSnapshot(
  items: readonly TimedItem[],
  cut: number,
  back: number,
  window = 128,
): TimedItem[] {
  const before = items.slice(0, cut);
  const upTo = items.slice(0, back);
  const events = upTo
    .map((timed) => timed.item)
    .filter((item): item is Record<string, unknown> => isRecord(item) && item["kind"] === "event");
  const lastEvent = events[events.length - 1];
  const hwm = typeof lastEvent?.["seq"] === "number" ? lastEvent["seq"] : 0;
  const snapshot = {
    kind: "snapshot",
    conversation: {},
    events: events.slice(-window).map((item) => {
      const envelope: Record<string, unknown> = Object.assign({}, item);
      delete envelope["kind"];
      return envelope;
    }),
    hwm,
    ...live(upTo),
  };
  const at = items[back]?.at ?? items[items.length - 1]?.at ?? 0;
  return [...before, { at, item: snapshot }, ...items.slice(back)];
}

/**
 * The stream of a phone that subscribed again with `after_seq` after an overflow: efrd
 * replays the events after the last one that the phone saw before item `cut`, at the
 * time of item `back`; the drafts in between are lost.
 */
export function resubscribeAfterSeq(
  items: readonly TimedItem[],
  cut: number,
  back: number,
): TimedItem[] {
  const before = items.slice(0, cut);
  const at = items[back]?.at ?? 0;
  const replayed = items
    .slice(cut, back)
    .filter((timed) => kindOf(timed.item) === "event")
    .map((timed) => ({ at, item: timed.item }));
  return [...before, ...replayed, ...items.slice(back)];
}

/** The `live` member of a snapshot after `items`: the text of the running message so far. */
function live(items: readonly TimedItem[]): { live?: Record<string, unknown> } {
  let turnId: string | undefined;
  let index = 0;
  const ends = new Map<string, number>();
  let ended = new Set<string>();
  let full = new Map<string, string>();
  for (const { item } of items) {
    if (!isRecord(item)) {
      continue;
    }
    const body = item["kind"] === "event" ? item["event"] : item["draft"];
    if (!isRecord(body)) {
      continue;
    }
    const turn = typeof body["turn_id"] === "string" ? body["turn_id"] : item["turn_id"];
    if (typeof turn !== "string") {
      continue;
    }
    const kind = body["kind"];
    if (kind === "text" || kind === "assistant_message_updated") {
      const at = Number(body["index"]);
      const key = `${turn}:${at}`;
      const delta = String(body["delta"]);
      const endByte = Number(body["offset"]) + utf8Length(delta);
      if (endByte > (ends.get(key) ?? 0)) {
        ends.set(key, endByte);
        const known = full.get(key) ?? "";
        const prefix = utf16IndexAtByte(known, Number(body["offset"]));
        full = new Map(full).set(
          key,
          prefix === undefined ? known : known.slice(0, prefix) + delta,
        );
      }
      turnId = turn;
      index = at;
    } else if (kind === "assistant_message_completed") {
      ended = new Set(ended).add(`${turn}:${String(body["index"])}`);
    } else if (typeof kind === "string" && kind.startsWith("turn_") && kind !== "turn_started") {
      if (turn === turnId) {
        turnId = undefined;
      }
    }
  }
  if (turnId === undefined) {
    return {};
  }
  const key = `${turnId}:${index}`;
  const text = ended.has(key) ? "" : (full.get(key) ?? "");
  return {
    live: {
      turn_id: turnId,
      after_seq: 0,
      index,
      text,
      text_bytes: utf8Length(text),
    },
  };
}

function kindOf(item: unknown): unknown {
  return isRecord(item) ? item["kind"] : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
