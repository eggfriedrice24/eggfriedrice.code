// Tells the app about the network path of the phone, from NetInfo
// (@react-native-community/netinfo, which uses NWPathMonitor on iOS). A change of the path,
// such as from Wi-Fi to cellular, leaves the old sockets dead, so the runtime of the
// machines tells each client to connect again at once.
//
// NetInfo tells whether the phone has a network. It does not tell whether a machine is
// reachable: on a phone with Tailscale off, the network is up and efrd is not. Each
// machine's client finds that out with its own connection.

import { addEventListener, type NetInfoState } from "@react-native-community/netinfo";

/** The network path of the phone, as the runtime compares it. */
export interface NetworkPath {
  /** The kind of network, such as `wifi`, `cellular`, `vpn` or `none`. */
  readonly type: string;
  /** True when the phone has a network. A state that is not known yet counts as true. */
  readonly connected: boolean;
}

/** The path of a NetInfo state. */
function pathOf(state: NetInfoState): NetworkPath {
  return { type: state.type, connected: state.isConnected !== false };
}

/**
 * Calls `listener` with the network path: once soon after the call, then on each change.
 * Returns the function that stops it.
 */
export function followNetwork(listener: (path: NetworkPath) => void): () => void {
  return addEventListener((state) => listener(pathOf(state)));
}

/**
 * Calls `setOnline` with true when the phone has a network, for the online manager of the
 * queries. Returns the function that stops it.
 */
export function followOnline(setOnline: (online: boolean) => void): () => void {
  return followNetwork((path) => setOnline(path.connected));
}
