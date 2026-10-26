//! Enrolled devices such as a phone: the facts that the daemon sends a remote
//! connection for its proof, the message that a device signs, the payload of the
//! pairing QR code and one row of `device.list`.
//!
//! The daemon owns the checks: the whois of the peer, the key, the signature and the
//! device rows. This module only fixes the bytes and the wire form, so the daemon, `efr`
//! and the phone app agree on them.

use std::fmt::Write as _;
use std::net::Ipv4Addr;

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{DaemonId, DeviceId, ScopeName, SecretText};

/// The domain tag at the start of every message that a device signs, so a signature
/// for efr can never pass for a signature of another protocol, or of a later version
/// of this one.
pub const PROVE_DOMAIN: &str = "efr-device-prove-v1";

/// The facts that a remote connection gets with its `hello` result, next to
/// `challenge`. The phone cannot read its own Tailscale node, so the daemon sends what
/// its whois found, and the device signs it.
///
/// Each value is a Tailscale stable id: a short ASCII string such as `nXXXXCNTRL` that
/// stays the same when the node changes its name or its addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProveContext {
    /// The stable id of the peer node, from the whois of the connection.
    pub peer_node: String,
    /// The stable id of the daemon's own node.
    pub daemon_node: String,
    /// The stable id of the daemon's tailnet.
    pub daemon_tailnet: String,
}

/// The message that a device signs for `device.prove`, see [`ProveMessage::to_bytes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProveMessage<'a> {
    /// The `challenge` of the `hello` result, as the string that the daemon sent.
    pub challenge: &'a str,
    /// The `daemon_id` of the `hello` result.
    pub daemon_id: DaemonId,
    /// The `protocol` of the `hello` result.
    pub protocol: u32,
    /// The device that proves its key.
    pub device_id: DeviceId,
    /// [`ProveContext::peer_node`].
    pub peer_node: &'a str,
    /// [`ProveContext::daemon_tailnet`].
    pub daemon_tailnet: &'a str,
}

impl ProveMessage<'_> {
    /// The bytes that the device signs with its P-256 key (ECDSA with SHA-256, the
    /// signature in DER), and that the daemon checks.
    ///
    /// The message is seven fields in this order: [`PROVE_DOMAIN`], `challenge`,
    /// `daemon_id`, `protocol`, `device_id`, `peer_node` and `daemon_tailnet`. Each
    /// field is its UTF-8 text after its length in bytes as a 4-byte big-endian
    /// number. An id is its lowercase hyphenated form, as on the wire, and `protocol`
    /// is its decimal form, such as `1`. The lengths make the message unambiguous: no
    /// two different sets of fields give the same bytes.
    ///
    /// ```
    /// use efr_protocol::{DaemonId, DeviceId, ProveMessage};
    ///
    /// let message = ProveMessage {
    ///     challenge: "abc",
    ///     daemon_id: DaemonId::from_uuid(uuid::Uuid::nil()),
    ///     protocol: 1,
    ///     device_id: DeviceId::from_uuid(uuid::Uuid::nil()),
    ///     peer_node: "n1",
    ///     daemon_tailnet: "T1",
    /// };
    /// let bytes = message.to_bytes();
    /// assert_eq!(&bytes[..4], &[0, 0, 0, 19]);
    /// assert_eq!(&bytes[4..23], b"efr-device-prove-v1");
    /// ```
    pub fn to_bytes(&self) -> Vec<u8> {
        let daemon_id = self.daemon_id.to_string();
        let protocol = self.protocol.to_string();
        let device_id = self.device_id.to_string();
        let fields = [
            PROVE_DOMAIN,
            self.challenge,
            &daemon_id,
            &protocol,
            &device_id,
            self.peer_node,
            self.daemon_tailnet,
        ];
        let mut bytes = Vec::with_capacity(fields.iter().map(|field| 4 + field.len()).sum());
        for field in fields {
            // NOTE: no field of a frame of at most 16 MiB comes near 4 GiB, so the
            // saturation never happens; it only keeps the function free of a panic.
            let length = u32::try_from(field.len()).unwrap_or(u32::MAX);
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes.extend_from_slice(field.as_bytes());
        }
        bytes
    }
}

/// What the pairing QR code holds: where the phone finds the daemon and the one-time
/// token that it sends with `device.enroll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairingQr {
    /// The daemon that the phone must find in its `hello` result before it sends the
    /// token.
    pub daemon_id: DaemonId,
    /// The daemon's IPv4 address on the tailnet. Never a loopback address.
    pub host: Ipv4Addr,
    /// The port of the daemon's tailnet listener.
    pub port: u16,
    /// The MagicDNS name of the daemon's node, a fallback for `host`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fqdn: Option<String>,
    /// The one-time token, as unpadded base64url. It is good for one enroll and expires
    /// after a few minutes.
    pub token: SecretText,
}

impl PairingQr {
    /// The text of the QR code: an `efr://pair` URL, such as
    /// `efr://pair?v=1&d=<daemon_id>&h=100.64.0.1&p=7878&f=<fqdn>&t=<token>`.
    ///
    /// The members in order: `v` (the version of the payload, `1`), `d` (`daemon_id`),
    /// `h` (`host`), `p` (`port`), `f` (`fqdn`, left out when absent) and `t`
    /// (`token`). Each value is percent-encoded: every byte that is not an ASCII
    /// letter, digit, `-`, `.`, `_` or `~` becomes `%` and two uppercase hex digits.
    /// The result holds the token, so it is a [`SecretText`].
    pub fn url(&self) -> SecretText {
        let mut url = String::from("efr://pair?v=1");
        let mut member = |name: &str, value: &str| {
            url.push('&');
            url.push_str(name);
            url.push('=');
            percent_encode(&mut url, value);
        };
        member("d", &self.daemon_id.to_string());
        member("h", &self.host.to_string());
        member("p", &self.port.to_string());
        if let Some(fqdn) = &self.fqdn {
            member("f", fqdn);
        }
        member("t", self.token.expose_secret());
        SecretText::new(url)
    }
}

/// Appends `value` to `out` with every byte outside the unreserved set of RFC 3986
/// percent-encoded.
fn percent_encode(out: &mut String, value: &str) {
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            // NOTE: writing to a String cannot fail.
            let _ = write!(out, "%{byte:02X}");
        }
    }
}

/// One enrolled device, as `device.list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceInfo {
    /// The device.
    pub device_id: DeviceId,
    /// The name that the device sent when it enrolled, such as `Anna's iPhone`.
    pub name: String,
    /// A short fingerprint of the device's public key, to compare with what the phone
    /// shows.
    pub key_fingerprint: String,
    /// The stable id of the Tailscale node that the device enrolled from. A connection
    /// from another node does not prove this device.
    pub node: String,
    /// What the device may do.
    pub scopes: Vec<ScopeName>,
    /// When the terminal confirmed the device.
    pub created_at: Timestamp,
    /// When the device last proved its key; absent when it never connected after it
    /// enrolled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<Timestamp>,
    /// When the device was revoked; absent while it may connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,
}

/// Why a pairing ended without a device, the `reason` of the `ended` item of
/// `device.pair_start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PairEnd {
    /// The user said no at the terminal.
    Refused,
    /// No phone enrolled, or the terminal did not confirm, before the token expired.
    Expired,
    /// Too many enrolls with a wrong token: the daemon closed the pairing.
    Attempts,
    /// A reason that this build does not know.
    #[serde(other)]
    Unknown,
}

impl PairEnd {
    /// Every reason, in the order of the enum.
    pub const ALL: [PairEnd; 4] =
        [PairEnd::Refused, PairEnd::Expired, PairEnd::Attempts, PairEnd::Unknown];
}

#[cfg(test)]
mod tests;
