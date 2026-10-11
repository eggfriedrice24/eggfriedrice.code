//! What the machine's `auto` sandbox can do, which changes the exits the engine finds.

/// What the `auto` sandbox of this machine supports beyond phase 1. Each later phase
/// turns a kind of exit into a routine call, or sends it to the classifier, so the
/// engine must know which parts run.
///
/// The default is phase 1: no network, no bus proxy, no undo, no home store, no mount
/// view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct AutoSupport {
    /// How a contained call reaches the network.
    pub egress: Egress,
    /// True when the filtered bus proxy runs (phase E): a read-only bus query is then
    /// routine, not a `bus` exit.
    pub bus_proxy: bool,
    /// True when snapshots and undo work and `sandbox.ask_destructive` is
    /// `unless_undoable` (phase B, efr's auto spec 10.7): a destructive part of a line,
    /// or a delete or move of a file tool, whose every target the snapshot or the home
    /// store covers ([`CallFacts::covered`](crate::CallFacts)) then runs with no
    /// review and no question. Without it every destructive part is an exit.
    pub undo: bool,
    /// True when efrd keeps the home store: it copies each home write before the call
    /// ([`ExitNeed::copies`](crate::ExitNeed)). Only then may a floor file edit that a
    /// user message names go to the classifier; without it, only the user approves one.
    pub floor_backup: bool,
    /// True when a contained call sees the host's mounts (phase D). Until then `df`
    /// and `findmnt` show the sandbox's mounts, so they are an `outside` exit.
    pub mount_view: bool,
}

/// How a contained call reaches the network.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Egress {
    /// No route (phase 1): every network need is a `host` exit.
    #[default]
    None,
    /// The efr proxy with its host allow list (phase D): the proxy decides, and a
    /// network need is no exit.
    Proxy,
}
