// Where the paired efrd is, and the URL of its tailnet listener.

/** The paired daemon and this device, as the pairing stored them. */
export interface Endpoint {
  /** The daemon that the pairing found; a hello from another daemon fails. */
  readonly daemonId: string;
  /** The daemon's IPv4 address on the tailnet, from the QR code. */
  readonly host: string;
  /** The port of the daemon's tailnet listener. */
  readonly port: number;
  /** The MagicDNS name of the daemon's node, from the QR code; for display only. */
  readonly fqdn?: string;
  /** The device id that `device.enroll` gave this phone. */
  readonly deviceId: string;
}

const IPV4 = /^(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}$/;

/** True for a dotted IPv4 address with no leading zeros, such as `100.64.0.1`. */
export function isIpv4(host: string): boolean {
  return IPV4.test(host);
}

/**
 * The URL of the listener, such as `ws://100.64.0.1:7419/`, or `undefined` when the
 * host is not an IPv4 address or the port is not from 1 to 65535.
 *
 * The app dials the IPv4 address and never the MagicDNS name: React Native sets the
 * Origin header from the URL, and efrd accepts only `http://<its tailnet IPv4>:<port>`.
 */
export function socketUrl(endpoint: Pick<Endpoint, "host" | "port">): string | undefined {
  const { host, port } = endpoint;
  if (!isIpv4(host) || !Number.isInteger(port) || port < 1 || port > 65_535) {
    return undefined;
  }
  return `ws://${host}:${port}/`;
}
