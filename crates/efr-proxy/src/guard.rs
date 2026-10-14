//! The address guard (DP7): an allow list of addresses that the proxy may connect to.
//!
//! [`classify`] first unwraps the IPv6 forms that carry an IPv4 address: IPv4-mapped
//! (`::ffff:0:0/96`), IPv4-compatible (`::/96`, but not `::` and `::1`) and NAT64
//! (`64:ff9b::/96`, and the `/96` form of `64:ff9b:1::/48`). It then gives the class
//! of the address:
//!
//! - an IPv4 address is [`AddressClass::Global`] only outside every block of the IANA
//!   IPv4 special-purpose address registry, and outside multicast and broadcast;
//! - an IPv6 address is `Global` only inside `2000::/3` and outside every block of the
//!   IANA IPv6 special-purpose address registry;
//! - an address of the host itself is [`AddressClass::Own`], and an address in the
//!   prefix of one of them (a neighbour on one of the host's links) is
//!   [`AddressClass::OnLink`], so a router, a printer or a NAS with a global address is
//!   refused too.
//!
//! [`Guard`] says which classes pass. The proxy refuses a name when any of its
//! addresses fails, and connects only to the addresses that it checked.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// What an address is, after the mapped, compatible and NAT64 forms are unwrapped.
/// `Global` means: outside every block of the IANA special-purpose registries, and
/// for IPv6 inside `2000::/3` (DP7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressClass {
    /// A public address that the guard passes.
    Global,
    /// `127.0.0.0/8` and `::1`.
    Loopback,
    /// RFC 1918: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`.
    Private,
    /// The shared address space of carrier-grade NAT, `100.64.0.0/10`.
    Shared,
    /// `169.254.0.0/16` (also cloud metadata services) and `fe80::/10`.
    LinkLocal,
    /// Unique local IPv6 addresses, `fc00::/7`.
    UniqueLocal,
    /// The old site-local IPv6 addresses, `fec0::/10`.
    SiteLocal,
    /// `224.0.0.0/4` and `ff00::/8`.
    Multicast,
    /// `0.0.0.0` and `::`.
    Unspecified,
    /// Every other block of the registries, broadcast, and IPv6 outside `2000::/3`.
    Reserved,
    /// The 6to4 and Teredo relays: `192.88.99.0/24`, `2002::/16`, `2001::/32`.
    Tunnel,
    /// The documentation blocks: `192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`,
    /// `2001:db8::/32`, `3fff::/20`.
    Documentation,
    /// An address on one of the host's links: in the prefix of one of its addresses.
    OnLink,
    /// An address of the host itself.
    Own,
}

/// One block of a special-purpose registry.
struct Block<T> {
    network: T,
    prefix: u8,
    class: AddressClass,
}

const fn v4(a: u8, b: u8, c: u8, d: u8, prefix: u8, class: AddressClass) -> Block<u32> {
    Block { network: u32::from_be_bytes([a, b, c, d]), prefix, class }
}

const fn v6(segments: [u16; 8], prefix: u8, class: AddressClass) -> Block<u128> {
    let s = segments;
    let network = (s[0] as u128) << 112
        | (s[1] as u128) << 96
        | (s[2] as u128) << 80
        | (s[3] as u128) << 64
        | (s[4] as u128) << 48
        | (s[5] as u128) << 32
        | (s[6] as u128) << 16
        | s[7] as u128;
    Block { network, prefix, class }
}

/// The IANA IPv4 special-purpose address registry, multicast and the rest of
/// `240.0.0.0/4` (which holds broadcast). The first block that holds an address wins,
/// so a small block comes before the block around it.
const V4_BLOCKS: &[Block<u32>] = {
    use AddressClass::*;
    &[
        v4(0, 0, 0, 0, 32, Unspecified),
        v4(0, 0, 0, 0, 8, Reserved),
        v4(10, 0, 0, 0, 8, Private),
        v4(100, 64, 0, 0, 10, Shared),
        v4(127, 0, 0, 0, 8, Loopback),
        v4(169, 254, 0, 0, 16, LinkLocal),
        v4(172, 16, 0, 0, 12, Private),
        // IETF protocol assignments, with DS-Lite, the PCP and TURN anycast addresses
        // and the NAT64 discovery names inside it.
        v4(192, 0, 0, 0, 24, Reserved),
        v4(192, 0, 2, 0, 24, Documentation),
        // AS112-v4.
        v4(192, 31, 196, 0, 24, Reserved),
        // AMT.
        v4(192, 52, 193, 0, 24, Reserved),
        // The old 6to4 relay anycast.
        v4(192, 88, 99, 0, 24, Tunnel),
        v4(192, 168, 0, 0, 16, Private),
        // Direct delegation AS112.
        v4(192, 175, 48, 0, 24, Reserved),
        // Benchmarking.
        v4(198, 18, 0, 0, 15, Reserved),
        v4(198, 51, 100, 0, 24, Documentation),
        v4(203, 0, 113, 0, 24, Documentation),
        v4(224, 0, 0, 0, 4, Multicast),
        // Reserved, with the limited broadcast address 255.255.255.255.
        v4(240, 0, 0, 0, 4, Reserved),
    ]
};

/// The IANA IPv6 special-purpose address registry, with the old site-local block,
/// multicast and the forms that [`unwrap_v4`] did not take.
const V6_BLOCKS: &[Block<u128>] = {
    use AddressClass::*;
    &[
        v6([0, 0, 0, 0, 0, 0, 0, 0], 128, Unspecified),
        v6([0, 0, 0, 0, 0, 0, 0, 1], 128, Loopback),
        // IPv4-mapped and IPv4-compatible forms that unwrap did not take.
        v6([0, 0, 0, 0, 0, 0xffff, 0, 0], 96, Reserved),
        v6([0, 0, 0, 0, 0, 0, 0, 0], 96, Reserved),
        // NAT64: the well-known prefix, and the local-use prefix in a form other than
        // its /96, whose IPv4 address does not sit in the last 32 bits.
        v6([0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96, Reserved),
        v6([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48, Reserved),
        // Discard-only.
        v6([0x100, 0, 0, 0, 0, 0, 0, 0], 64, Reserved),
        // Teredo, then the rest of the IETF protocol assignments (ORCHID, AMT, AS112,
        // benchmarking, the anycast addresses).
        v6([0x2001, 0, 0, 0, 0, 0, 0, 0], 32, Tunnel),
        v6([0x2001, 0, 0, 0, 0, 0, 0, 0], 23, Reserved),
        v6([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32, Documentation),
        // 6to4.
        v6([0x2002, 0, 0, 0, 0, 0, 0, 0], 16, Tunnel),
        // Direct delegation AS112.
        v6([0x2620, 0x4f, 0x8000, 0, 0, 0, 0, 0], 48, Reserved),
        v6([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20, Documentation),
        v6([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7, UniqueLocal),
        v6([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10, LinkLocal),
        v6([0xfec0, 0, 0, 0, 0, 0, 0, 0], 10, SiteLocal),
        v6([0xff00, 0, 0, 0, 0, 0, 0, 0], 8, Multicast),
    ]
};

/// The class of `addr` (DP7). `own` holds the host's addresses with their prefix
/// lengths (the address and the netmask of `getifaddrs`).
pub fn classify(addr: IpAddr, own: &[(IpAddr, u8)]) -> AddressClass {
    let addr = unwrap_v4(addr);
    if own.iter().any(|(own, _)| unwrap_v4(*own) == addr) {
        return AddressClass::Own;
    }
    let class = match addr {
        IpAddr::V4(v4) => class_v4(v4),
        IpAddr::V6(v6) => class_v6(v6),
    };
    let neighbour = matches!(
        class,
        AddressClass::Global
            | AddressClass::Private
            | AddressClass::Shared
            | AddressClass::UniqueLocal
    ) && own.iter().any(|(own, prefix)| same_prefix(addr, *own, *prefix));
    if neighbour { AddressClass::OnLink } else { class }
}

/// The IPv4 address inside an IPv4-mapped, IPv4-compatible or NAT64 form; any other
/// address as it is.
pub(crate) fn unwrap_v4(addr: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = addr else {
        return addr;
    };
    let s = v6.segments();
    let low = Ipv4Addr::from_bits((u32::from(s[6]) << 16) | u32::from(s[7]));
    let mapped = s[..5] == [0; 5] && s[5] == 0xffff;
    let compatible = s[..6] == [0; 6] && !matches!(v6, Ipv6Addr::UNSPECIFIED | Ipv6Addr::LOCALHOST);
    let nat64 = s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] || s[..6] == [0x64, 0xff9b, 1, 0, 0, 0];
    if mapped || compatible || nat64 { IpAddr::V4(low) } else { addr }
}

fn class_v4(addr: Ipv4Addr) -> AddressClass {
    let bits = addr.to_bits();
    V4_BLOCKS
        .iter()
        .find(|block| bits & mask_v4(block.prefix) == block.network)
        .map_or(AddressClass::Global, |block| block.class)
}

fn class_v6(addr: Ipv6Addr) -> AddressClass {
    let bits = addr.to_bits();
    if let Some(block) =
        V6_BLOCKS.iter().find(|block| bits & mask_v6(block.prefix) == block.network)
    {
        return block.class;
    }
    // Global unicast is 2000::/3; everything else is reserved.
    if bits >> 125 == 1 { AddressClass::Global } else { AddressClass::Reserved }
}

/// True when `addr` lies in the prefix of `own`, of the same family.
fn same_prefix(addr: IpAddr, own: IpAddr, prefix: u8) -> bool {
    match (addr, own) {
        (IpAddr::V4(addr), IpAddr::V4(own)) => {
            let mask = mask_v4(prefix.min(32));
            addr.to_bits() & mask == own.to_bits() & mask
        }
        (IpAddr::V6(addr), IpAddr::V6(own)) => {
            let mask = mask_v6(prefix.min(128));
            addr.to_bits() & mask == own.to_bits() & mask
        }
        _ => false,
    }
}

fn mask_v4(prefix: u8) -> u32 {
    u32::MAX.checked_shl(32 - u32::from(prefix.min(32))).unwrap_or(0)
}

fn mask_v6(prefix: u8) -> u128 {
    u128::MAX.checked_shl(128 - u32::from(prefix.min(128))).unwrap_or(0)
}

/// Which classes may pass. [`Guard::strict`] passes only [`AddressClass::Global`], and
/// for a `private = true` entry also `Private`, `Shared`, `UniqueLocal` and `OnLink`;
/// never loopback, link-local or an own address. [`Guard::with_loopback`] is for tests:
/// it also passes `Loopback`, so a test can reach its own server on `127.0.0.1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guard {
    loopback: bool,
}

impl Guard {
    /// The guard of efrd.
    pub const fn strict() -> Guard {
        Guard { loopback: false }
    }

    /// The strict guard that also passes loopback addresses. Only tests use it.
    pub const fn with_loopback() -> Guard {
        Guard { loopback: true }
    }

    /// True when an address of `class` may pass; `private_entry` is true for a host of
    /// a `private = true` entry.
    pub fn passes(&self, class: AddressClass, private_entry: bool) -> bool {
        match class {
            AddressClass::Global => true,
            AddressClass::Private
            | AddressClass::Shared
            | AddressClass::UniqueLocal
            | AddressClass::OnLink => private_entry,
            AddressClass::Loopback => self.loopback,
            AddressClass::LinkLocal
            | AddressClass::SiteLocal
            | AddressClass::Multicast
            | AddressClass::Unspecified
            | AddressClass::Reserved
            | AddressClass::Tunnel
            | AddressClass::Documentation
            | AddressClass::Own => false,
        }
    }
}

#[cfg(test)]
mod tests;
