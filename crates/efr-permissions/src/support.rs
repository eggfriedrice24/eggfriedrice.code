//! What the machine's `auto` sandbox can do, which changes the exits the engine finds.

/// What the `auto` sandbox of this machine supports beyond phase 1. Each later phase
/// turns a kind of exit into a routine call, so the engine must know which parts run.
///
/// The default is phase 1: no network, no bus proxy, no undo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct AutoSupport {
    /// How a contained call reaches the network.
    pub egress: Egress,
    /// True when the filtered bus proxy runs (phase 5): a read-only bus query is then
    /// routine, not a `bus` exit.
    pub bus_proxy: bool,
    /// True when snapshots and undo work (phase 4).
    // NOTE: phase 4 reads it to make a destructive line routine when the snapshot
    // covers its targets. Phase 1 keeps every destructive line an exit.
    pub undo: bool,
}

/// How a contained call reaches the network.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Egress {
    /// No route (phase 1): every network need is a `host` exit.
    #[default]
    None,
    /// The efr proxy with its host allow list (phase 2): the proxy decides, and a
    /// network need is no exit.
    Proxy,
}
