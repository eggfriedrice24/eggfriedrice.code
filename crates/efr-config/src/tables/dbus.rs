//! `[dbus]`: the filtered bus proxy of the `auto` mode (phase E, efr's auto spec 11.1).

use efr_permissions::BusRule;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[dbus]`: the filtered message bus of the `auto` sandbox. Every key applies from the
/// next call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct DbusSettings {
    /// `true`: in `auto`, the read forms of `systemctl`, `loginctl`, `hostnamectl`,
    /// `timedatectl` and `localectl`, and `notify-send`, run in the sandbox with no
    /// question, through a filtered bus that lets through only a fixed list of methods.
    /// `false`: they need the whole bus, which the reviewer approves for the system bus
    /// and only you for the session bus. efrd turns the filter off when
    /// `xdg-dbus-proxy` is missing or fails its self-test. A change applies to new
    /// calls; a running background job keeps the bus rules of its start.
    pub proxy: bool,
    /// More bus methods that the filter lets through, each a table: `bus` (`system` or
    /// `session`), `name` (the service, such as `org.freedesktop.timedate1`) and `call`
    /// (a list of `IFACE.MEMBER` or `IFACE.MEMBER@PATH`; a PATH may end in `/*`). They
    /// add to the built-in read methods. For systemd, logind, hostnamed, timedated,
    /// localed and timesyncd a method must start with `Get`, `List`, `Can` or `Lookup`.
    /// For other services efr refuses a method that starts with a word that changes the
    /// machine, such as `Start` or `Set`, but it cannot tell every read from a write:
    /// you own each method that you add. Only an editor changes them.
    pub allow: Vec<BusRule>,
}

impl Default for DbusSettings {
    /// The filter is on, with no added methods: from the end of phase E (DE17), after
    /// its tests passed on this machine and in CI's VM.
    fn default() -> Self {
        DbusSettings { proxy: true, allow: Vec::new() }
    }
}
