//! `device.pair_start`: open a pairing for a new device, for `efr device add`.

use jiff::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{DeviceId, PairEnd, PairingId, PairingQr};

/// The params of `device.pair_start`, a streaming admin method (Unix socket only).
///
/// The daemon makes a one-time token and sends the `started` item with the QR code.
/// When a phone sends that token with `device.enroll`, the daemon sends `pending`
/// with the facts of the phone, and the terminal answers with `device.pair_confirm`.
/// The stream then sends `paired` or `ended` and ends. A cancel of the request, or the
/// end of the connection, ends the pairing: the token is then good for nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevicePairStart {
    /// The name to give the device. Absent: the name that the phone sends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One item of a `device.pair_start` stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DevicePairStartItem {
    /// The pairing is open: show the QR code. Always the first item.
    Started {
        /// The pairing, for `device.pair_confirm`.
        pairing_id: PairingId,
        /// When the token expires.
        expires_at: Timestamp,
        /// What the QR code holds; [`PairingQr::url`] is its text.
        qr: PairingQr,
    },
    /// A phone sent the token. The daemon waits for `device.pair_confirm`. The user
    /// compares these facts with the phone before saying yes.
    Pending {
        /// The pairing.
        pairing_id: PairingId,
        /// The name that the phone sent.
        device_name: String,
        /// The name of the phone's Tailscale node.
        node_name: String,
        /// The login of the Tailscale user of the phone's node, such as
        /// `anna@example.com`. For display only.
        user_login: String,
        /// The fingerprint of the phone's key, which the phone shows too.
        key_fingerprint: String,
        /// The operating system of the phone's node, such as `iOS`, when Tailscale
        /// knows it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        os: Option<String>,
    },
    /// The terminal said yes and the device is enrolled. The last item.
    Paired {
        /// The pairing.
        pairing_id: PairingId,
        /// The new device.
        device_id: DeviceId,
    },
    /// The pairing ended without a device. The last item.
    Ended {
        /// The pairing.
        pairing_id: PairingId,
        /// Why.
        reason: PairEnd,
    },
}
