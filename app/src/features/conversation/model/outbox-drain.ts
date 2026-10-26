// The drain of the outbox for one machine. The supervisor of the machine makes one, calls
// `start` on each `ready` of its client, and `stop` when the connection goes down. While
// it runs, the drain sends each due entry (`drainPlan` in outbox.ts) with `prompt.send`,
// and sends again when the outbox changes or a wait ends.
//
// - A lost connection (a ConnectionError) makes the entry pending again with no wait, and
//   holds the drain until the next `start`, so a dead client cannot make a loop of
//   sends that fail at once.
// - `busy` and `internal` are retryable: the entry waits `retry_after_ms` of the error,
//   else the backoff ladder of src/lib/net/backoff.ts with a cap of 16 s (t3code), and
//   fails after MAX_SEND_ATTEMPTS. An error that is not efrd's counts as retryable too.
// - Every other error of efrd fails the entry: a resend with the same command id gets
//   the same answer.
//
// Pure TypeScript: the caller gives the send function, the clock and the random value.

import { backoffDelayMs, type BackoffConfig } from "@/lib/net/backoff";
import type { Clock, Timer } from "@/lib/net/clock";
import { ConnectionError, ServerError, retryAfterMs } from "@/lib/net/errors";
import type { CommandId, DaemonId, PromptSend, PromptSendResult } from "@/lib/protocol/generated";
import { drainPlan, sendParams, type Outbox, type OutboxEntry, type OutboxError } from "./outbox";

/** The wait between the attempts of one entry: 0.5 s, then twice as long, at most 16 s. */
export const OUTBOX_BACKOFF: BackoffConfig = { baseMs: 250, maxMs: 16_000, resetAfterMs: 0 };

/** The codes of efrd after which a resend can work. */
const RETRYABLE_CODES: ReadonlySet<string> = new Set(["busy", "internal"]);

/** What the drain needs. */
export interface OutboxDrainOptions {
  readonly outbox: Outbox;
  /** The daemon id of the machine. The drain sends only its entries. */
  readonly machine: DaemonId;
  /** Sends `prompt.send` on the client of the machine. */
  readonly send: (params: PromptSend) => Promise<PromptSendResult>;
  readonly clock: Clock;
  /** A random value in [0, 1) for the backoff ladder. */
  readonly random: () => number;
  readonly backoff?: BackoffConfig;
  /** Called after efrd took an entry, with its result. */
  readonly onSent?: (entry: OutboxEntry, result: PromptSendResult) => void;
}

/** The drain of one machine. */
export interface OutboxDrain {
  /** The client is ready: send the due entries, and each new one. */
  start(): void;
  /** The client is not ready: send nothing more. A send in flight still ends. */
  stop(): void;
  /** True while the drain runs and no lost connection holds it. */
  readonly running: boolean;
}

/** The error of an entry for a failure of its send. */
function outboxError(error: unknown): OutboxError {
  if (error instanceof ServerError) {
    return { code: error.body.code, message: error.body.message };
  }
  if (error instanceof ConnectionError) {
    return { code: "connection", message: error.message };
  }
  if (error instanceof Error) {
    return { code: "client", message: error.message };
  }
  return { code: "client", message: String(error) };
}

/** Makes the drain of `options.machine`. It does nothing before `start`. */
export function createOutboxDrain(options: OutboxDrainOptions): OutboxDrain {
  const { outbox, machine, clock } = options;
  const backoff = options.backoff ?? OUTBOX_BACKOFF;
  let running = false;
  let unsubscribe: (() => void) | undefined;
  let timer: Timer | undefined;
  let pumping = false;
  let again = false;
  // The number of the current start; a send keeps the number of the start that sent it.
  let generation = 0;

  const halt = (): void => {
    running = false;
    unsubscribe?.();
    unsubscribe = undefined;
    timer?.cancel();
    timer = undefined;
  };

  const settle = (commandId: CommandId, error: unknown, sentIn: number): void => {
    const reason = outboxError(error);
    if (error instanceof ConnectionError) {
      // The client is down. The supervisor calls `start` on its next ready. The drain
      // halts first: the release changes the outbox, and a running drain would send
      // the entry again at once. A send of an older start is different: its socket
      // closed after a new connection came up, so the drain sends again on the new one.
      if (sentIn === generation) {
        halt();
      }
      outbox.release(commandId, reason);
      return;
    }
    if (error instanceof ServerError && !RETRYABLE_CODES.has(error.body.code)) {
      outbox.fail(commandId, reason);
      return;
    }
    const entry = outbox.store.get().entries.find((held) => held.commandId === commandId);
    const attempt = entry?.attempts ?? 0;
    const asked = error instanceof ServerError ? retryAfterMs(error.body) : undefined;
    const wait = asked ?? backoffDelayMs(attempt, options.random(), backoff);
    outbox.retryLater(commandId, reason, clock.now() + wait);
  };

  const sendOne = (entry: OutboxEntry): void => {
    const params = sendParams(outbox.store.get(), entry);
    if (!outbox.begin(entry.commandId)) {
      return;
    }
    const sentIn = generation;
    const run = async (): Promise<void> => {
      let result: PromptSendResult;
      try {
        // A send that throws at once is a failed send too.
        result = await options.send(params);
      } catch (error) {
        settle(entry.commandId, error, sentIn);
        return;
      }
      outbox.succeed(entry.commandId, result);
      options.onSent?.(entry, result);
    };
    void run().catch(() => undefined);
  };

  const pump = (): void => {
    if (!running) {
      return;
    }
    // A send changes the outbox, which calls pump again; that run waits for this one.
    if (pumping) {
      again = true;
      return;
    }
    pumping = true;
    try {
      for (;;) {
        again = false;
        timer?.cancel();
        timer = undefined;
        const plan = drainPlan(outbox.store.get(), machine, clock.now());
        for (const entry of plan.due) {
          if (!running) {
            return;
          }
          sendOne(entry);
        }
        if (running && plan.wakeAt !== undefined) {
          timer = clock.setTimer(plan.wakeAt - clock.now(), pump);
        }
        if (!again || !running) {
          break;
        }
      }
    } finally {
      pumping = false;
    }
  };

  return {
    start(): void {
      if (running) {
        pump();
        return;
      }
      running = true;
      generation += 1;
      const subscription = outbox.store.subscribe(() => pump());
      unsubscribe = () => subscription.unsubscribe();
      pump();
    },
    stop(): void {
      halt();
    },
    get running(): boolean {
      return running;
    },
  };
}
