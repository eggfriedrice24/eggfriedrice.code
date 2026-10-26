// When a change of the phone's network path must make every client connect again. NetInfo
// reports the path once at the start and then on each change; a socket that was open on
// the old path is dead after a change (for example Wi-Fi to cellular), and iOS can take a
// long time to report it. Pure, for the tests.

/** The network path of the phone (src/lib/native/netinfo.ts gives it). */
export interface NetworkPath {
  /** The kind of network, such as `wifi`, `cellular`, `vpn` or `none`. */
  readonly type: string;
  readonly connected: boolean;
}

/**
 * True when the clients must connect again at once after the path went from `previous`
 * to `next`. The first report (`previous` undefined) is no change. A loss of the network is
 * no reason either: there is nothing to connect to, and the clients find out by themselves.
 */
export function pathChanged(previous: NetworkPath | undefined, next: NetworkPath): boolean {
  if (previous === undefined || !next.connected) {
    return false;
  }
  return previous.type !== next.type || !previous.connected;
}
