// The message that a device signs for `device.prove`. It is a copy of
// `ProveMessage::to_bytes` in crates/efr-protocol/src/device.rs, and the test vector of
// efr-protocol (fixtures/v1/device_prove_message.json) checks that both give the same
// bytes.
//
// The message is seven fields in this order: the domain tag, `challenge`, `daemon_id`,
// `protocol`, `device_id`, `peer_node` and `daemon_tailnet`. Each field is its UTF-8
// text after its length in bytes as a 4-byte big-endian number. `protocol` is its
// decimal form, such as `1`. Ids are the strings that efrd sent, in their lowercase
// hyphenated form. `daemon_node` of the prove context is not signed.

import { utf8Bytes } from "./bytes";
import type { HelloResult } from "./generated";

/** The domain tag at the start of every signed message (`PROVE_DOMAIN`). */
export const PROVE_DOMAIN = "efr-device-prove-v1";

/** The fields of the signed message. */
export interface ProveFields {
  /** The `challenge` of the `hello` result, as efrd sent it. */
  readonly challenge: string;
  /** The `daemon_id` of the `hello` result. */
  readonly daemon_id: string;
  /** The `protocol` of the `hello` result. */
  readonly protocol: number;
  /** The device that proves its key. */
  readonly device_id: string;
  /** `prove.peer_node` of the `hello` result. */
  readonly peer_node: string;
  /** `prove.daemon_tailnet` of the `hello` result. */
  readonly daemon_tailnet: string;
}

/**
 * The fields to sign for `deviceId` on the connection that got `hello`, or `undefined`
 * when the result has no prove context (it does not come from the tailnet listener).
 */
export function proveFields(hello: HelloResult, deviceId: string): ProveFields | undefined {
  if (hello.prove === undefined || hello.prove === null) {
    return undefined;
  }
  return {
    challenge: hello.challenge,
    daemon_id: hello.daemon_id,
    protocol: hello.protocol,
    device_id: deviceId,
    peer_node: hello.prove.peer_node,
    daemon_tailnet: hello.prove.daemon_tailnet,
  };
}

/** The bytes that the device signs (ECDSA P-256 with SHA-256, the signature in DER). */
export function proveMessage(fields: ProveFields): Uint8Array {
  const parts = [
    PROVE_DOMAIN,
    fields.challenge,
    fields.daemon_id,
    String(fields.protocol),
    fields.device_id,
    fields.peer_node,
    fields.daemon_tailnet,
  ].map((field) => utf8Bytes(field));
  const size = parts.reduce((total, part) => total + 4 + part.length, 0);
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const part of parts) {
    const length = part.length;
    bytes[at] = (length >>> 24) & 0xff;
    bytes[at + 1] = (length >>> 16) & 0xff;
    bytes[at + 2] = (length >>> 8) & 0xff;
    bytes[at + 3] = length & 0xff;
    bytes.set(part, at + 4);
    at += 4 + length;
  }
  return bytes;
}
