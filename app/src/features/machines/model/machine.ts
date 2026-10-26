// A paired machine as the registry lists it, and the endpoint of its client. Pure, for the
// tests.

import type { Endpoint } from "@/lib/net/endpoint";
import { machineName, type PairedDevice } from "@/features/pairing/model/paired-device";
import type { MachineId } from "./machine-ref";

/** One paired machine. */
export interface Machine {
  /** The daemon id. */
  readonly id: MachineId;
  /** The short name, such as `desktop` (the first label of the MagicDNS name). */
  readonly name: string;
  /** The MagicDNS name of the machine's node, when the pairing code had one. */
  readonly fqdn: string | undefined;
  /** The pairing record. */
  readonly device: PairedDevice;
  /** True when the device key is gone: the machine needs a re-pair. */
  readonly keyMissing: boolean;
}

/** The machine of a pairing record. */
export function machineOf(device: PairedDevice, keyMissing: boolean): Machine {
  return {
    id: device.daemonId,
    name: machineName(device),
    fqdn: device.fqdn,
    device,
    keyMissing,
  };
}

/** The endpoint of the client of a machine. */
export function endpointOf(device: PairedDevice): Endpoint {
  const endpoint: Endpoint = {
    daemonId: device.daemonId,
    host: device.host,
    port: device.port,
    deviceId: device.deviceId,
  };
  return device.fqdn === undefined ? endpoint : { ...endpoint, fqdn: device.fqdn };
}

/** The `host:port` of a machine, for the words of the connection states. */
export function endpointLabel(device: PairedDevice): string {
  return `${device.host}:${device.port}`;
}
