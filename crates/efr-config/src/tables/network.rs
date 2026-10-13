//! `[network]`: what network a contained call of the `auto` mode sees (efr's auto
//! spec, section 8.8, phase D1).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[network]`: the network of the `auto` sandbox. Every key applies from the next
/// call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct NetworkSettings {
    /// `true`: in `auto`, `ss`, the read forms of `ip`, `nstat` and reads of `/proc/net`
    /// run in the sandbox with your machine's network view and no network socket, with
    /// no question. `false`: they go to the reviewer, as a look at the host's network.
    pub netview: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        NetworkSettings { netview: true }
    }
}
