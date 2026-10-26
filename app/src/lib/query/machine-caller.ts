// What the query and mutation factories need from the runtime: one request to one
// machine. The runtime of the machines feature gives it; the tests give a fake
// (./testing.ts). The factories never see a client or a socket.

import type { UnaryMethodName } from "@/lib/net/connection";
import type { MethodParams } from "@/lib/protocol/generated";
import type { CallResults } from "@/lib/protocol/wire";

/** Sends one request to one machine. */
export interface MachineCaller {
  /**
   * Runs `method` on the ready connection of `machine` (its `daemonId`). It rejects with
   * a `ConnectionError` of kind `offline` when the machine has no ready connection, and
   * with a `ServerError` when efrd answers with an error. When `signal` aborts, the
   * promise rejects; efrd can still run the request.
   */
  call<M extends UnaryMethodName>(
    machine: string,
    method: M,
    params: MethodParams[M],
    signal?: AbortSignal,
  ): Promise<CallResults[M]>;
  /**
   * True while a watch of the lists of `machine` is live (`conversations.watch`), so its
   * lists need not poll. Absent: the lists always poll.
   */
  watches?(machine: string): boolean;
}
