//! `[remote]`: efrd's listener for the efr phone app on this machine's Tailscale
//! address.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The port of the remote listener when nothing names one. It is below the ephemeral
/// range of Linux (32768 to 60999), so an outgoing connection never holds it.
pub const DEFAULT_REMOTE_PORT: u16 = 7419;

/// The LocalAPI socket of tailscaled when nothing names one. On most distributions
/// `/var/run` is a link to `/run`.
pub const DEFAULT_TAILSCALED_SOCKET: &str = "/var/run/tailscale/tailscaled.sock";

/// `[remote]`: the listener for the efr phone app. efrd listens only on this machine's
/// Tailscale IPv4 address, never on another interface, and admits a connection only
/// from a paired device of the same Tailscale user. Every key needs a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct RemoteSettings {
    /// `true` lets efrd listen for the efr phone app on this machine's Tailscale IPv4
    /// address. It needs tailscaled 1.100 or later with a TUN device (not userspace
    /// networking). `false` opens no port. Then pair a phone with `efr device add`.
    /// Needs a restart.
    pub enabled: bool,
    /// The TCP port of the listener on the Tailscale address, from 1024 to 65535.
    /// Needs a restart.
    pub port: u16,
    /// The absolute path of tailscaled's LocalAPI socket, which efrd asks who each
    /// peer is. Needs a restart.
    pub tailscaled_socket: PathBuf,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        RemoteSettings {
            enabled: false,
            port: DEFAULT_REMOTE_PORT,
            tailscaled_socket: PathBuf::from(DEFAULT_TAILSCALED_SOCKET),
        }
    }
}
