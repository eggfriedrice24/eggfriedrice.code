// The schemas of the error data that the app reads. The protocol schema types `data` as
// any value, because its shape depends on the code (docs/protocol.md, "Errors"). Each
// schema below reads one member, so a bad member does not hide a good one.

import { z } from "zod";

/** The wait in milliseconds that a `busy` error asks for. */
export const retryAfterMsDataSchema = z.object({ retry_after_ms: z.number().nonnegative() });

/** The wait in seconds that a `busy` error asks for, as efrd sends it while it starts. */
export const retryAfterSecsDataSchema = z.object({ retry_after_secs: z.number().nonnegative() });

/** The mark of a proof from another Tailscale node: the phone must pair again. */
export const rePairDataSchema = z.object({ re_pair: z.literal(true) });
