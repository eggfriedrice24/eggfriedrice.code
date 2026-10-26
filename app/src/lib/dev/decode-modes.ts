// The decoders of the three modes of the decode bench (decode-bench.ts), and whether zod
// can compile its object parsers on this engine at all. Pure.

import { util } from "zod/v4/core";

import { decodeSubscribeItem } from "@/lib/protocol/envelope";
import { decodeSubscribeItem as handDecodeSubscribeItem } from "@/lib/protocol/hand-guard";
import type { BenchDecoder, BenchMode } from "./decode-bench";
import { schemaDecoder } from "./schema-decoder";

/** The decoder of each mode. */
export const BENCH_DECODERS: Readonly<Record<BenchMode, BenchDecoder>> = {
  zod: decodeSubscribeItem,
  jitless: schemaDecoder({ jitless: true }),
  hand: handDecodeSubscribeItem,
};

/**
 * True when zod can make functions with `new Function` here, so the `zod` mode uses its
 * compiled object parsers. When it is false, the `zod` and `jitless` modes do the same
 * work.
 */
export function zodCompiles(): boolean {
  return util.allowsEval.value;
}
