// The connection of the phone to one machine, as the supervisor keeps it in its atom: the
// state of the client, the view that the screens read, and when the machine was last seen.
// Pure, for the tests.

import type { ClientState } from "@/lib/net/client";
import type { HelloResult, ScopeName } from "@/lib/protocol/generated";
import { connectionView, type ConnectionView } from "./connection-view";

/** The connection to one machine. */
export interface MachineConnection {
  readonly client: ClientState;
  readonly view: ConnectionView;
  /**
   * When the phone was last connected to the machine (clock time, ms): the time the
   * connection came up, or the time it went down. Undefined until the first connection
   * of this run. The offline line says "seen 13:58".
   */
  readonly seenAt: number | undefined;
}

/** What efrd said about itself on the last connection. */
export interface MachineStatus {
  /** efrd's build version, such as `0.0.5`. */
  readonly version: string;
  /** efrd's protocol version. */
  readonly protocol: number;
  /** What this phone may do on the machine. */
  readonly scopes: readonly ScopeName[];
  /** The clock time of the connection that gave this status. */
  readonly seenAt: number;
}

/** The connection before the first state of the client. `endpoint` is `host:port`. */
export function initialConnection(client: ClientState, endpoint: string): MachineConnection {
  return { client, view: connectionView(client, endpoint), seenAt: undefined };
}

/** The connection of a machine whose device key is gone: the user must pair it again. */
export function keylessConnection(): MachineConnection {
  return {
    client: { state: "off" },
    view: { kind: "repair_needed", reason: "other" },
    seenAt: undefined,
  };
}

/** The connection after the client went to `client` at `now`. */
export function nextConnection(
  previous: MachineConnection,
  client: ClientState,
  endpoint: string,
  now: number,
): MachineConnection {
  const seen = client.state === "ready" || previous.client.state === "ready";
  return {
    client,
    view: connectionView(client, endpoint),
    seenAt: seen ? now : previous.seenAt,
  };
}

/** The status of a ready connection, or undefined when the client is not ready. */
export function statusOf(connection: MachineConnection): MachineStatus | undefined {
  const { client, seenAt } = connection;
  if (client.state !== "ready" || seenAt === undefined) {
    return undefined;
  }
  return statusFromHello(client.hello, client.scopes, seenAt);
}

function statusFromHello(
  hello: HelloResult,
  scopes: readonly ScopeName[],
  seenAt: number,
): MachineStatus {
  return { version: hello.version, protocol: hello.protocol, scopes, seenAt };
}
