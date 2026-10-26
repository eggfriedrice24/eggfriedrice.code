// Fakes for the tests of the query and mutation factories: a machine caller that
// records each request and answers with the handler of a test, and a storage in memory.
// Only test files import this module.

import type { AsyncStorage } from "@tanstack/react-query-persist-client";

import type { UnaryMethodName } from "@/lib/net/connection";
import { ConnectionError } from "@/lib/net/errors";
import type { MethodParams, MethodResults } from "@/lib/protocol/generated";
import type { MachineCaller } from "./machine-caller";

/** One request that the fake caller received. */
export interface FakeCall {
  readonly machine: string;
  readonly method: UnaryMethodName;
  readonly params: unknown;
  readonly signal: AbortSignal | undefined;
}

type Handler<M extends UnaryMethodName> = (
  params: MethodParams[M],
  machine: string,
) => MethodResults[M] | Promise<MethodResults[M]>;

/** The answers of a fake caller, by method. */
export type FakeHandlers = { readonly [M in UnaryMethodName]?: Handler<M> };

/**
 * A caller whose answers come from the handlers of a test. A method without a handler
 * fails with a `ConnectionError` of kind `offline`, as a machine without a connection
 * does.
 */
export class FakeCaller implements MachineCaller {
  readonly calls: FakeCall[] = [];
  private handlers: FakeHandlers;

  constructor(handlers: FakeHandlers = {}) {
    this.handlers = handlers;
  }

  /** Answers the methods of `handlers` with them from now on; the others stay. */
  answer(handlers: FakeHandlers): void {
    this.handlers = { ...this.handlers, ...handlers };
  }

  /** The calls of `method`, oldest first. */
  callsOf(method: UnaryMethodName): FakeCall[] {
    return this.calls.filter((call) => call.method === method);
  }

  call<M extends UnaryMethodName>(
    machine: string,
    method: M,
    params: MethodParams[M],
    signal?: AbortSignal,
  ): Promise<MethodResults[M]> {
    this.calls.push({ machine, method, params, signal });
    const handler = this.handlers[method];
    if (handler === undefined) {
      return Promise.reject(new ConnectionError("offline", "the phone is not connected to efrd"));
    }
    try {
      return Promise.resolve(handler(params, machine));
    } catch (error) {
      return Promise.reject(error instanceof Error ? error : new Error(String(error)));
    }
  }
}

/** A storage in memory, with the writes it received. */
export class MemoryStorage implements AsyncStorage {
  readonly values = new Map<string, string>();
  writes = 0;

  getItem(key: string): Promise<string | null> {
    return Promise.resolve(this.values.get(key) ?? null);
  }

  setItem(key: string, value: string): Promise<void> {
    this.writes += 1;
    this.values.set(key, value);
    return Promise.resolve();
  }

  removeItem(key: string): Promise<void> {
    this.values.delete(key);
    return Promise.resolve();
  }
}

/** A promise that settles after the pending promise jobs and timers of this turn. */
export function settle(): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
}
