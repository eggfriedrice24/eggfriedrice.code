use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::AddressClass::*;
use super::*;

/// Addresses outside both special-purpose registries, as the test kit of the
/// integration tests names them.
const GLOBAL_V4: &str = "11.0.0.1";
const GLOBAL_V4_OWN: &str = "11.0.1.1";
const GLOBAL_V6: &str = "2a00:0:0:1::1";
const GLOBAL_V6_OWN: &str = "2a00:0:0:2::1";

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[test]
fn classify_special_purpose_tables() {
    // One row per block of the IANA IPv4 and IPv6 special-purpose registries, plus
    // multicast, the old site-local block and the unwrapped forms.
    let rows = [
        // IPv4.
        ("0.0.0.0", Unspecified),
        ("0.1.2.3", Reserved),
        ("10.0.0.1", Private),
        ("10.255.255.255", Private),
        ("100.64.0.1", Shared),
        ("100.127.255.255", Shared),
        ("100.128.0.1", Global),
        ("127.0.0.1", Loopback),
        ("127.255.0.1", Loopback),
        ("169.254.169.254", LinkLocal),
        ("172.16.0.1", Private),
        ("172.31.255.255", Private),
        ("172.32.0.1", Global),
        ("192.0.0.0", Reserved),
        ("192.0.0.8", Reserved),
        ("192.0.0.9", Reserved),
        ("192.0.0.10", Reserved),
        ("192.0.0.170", Reserved),
        ("192.0.2.1", Documentation),
        ("192.31.196.1", Reserved),
        ("192.52.193.1", Reserved),
        ("192.88.99.1", Tunnel),
        ("192.168.0.1", Private),
        ("192.175.48.1", Reserved),
        ("198.18.0.1", Reserved),
        ("198.19.255.255", Reserved),
        ("198.20.0.1", Global),
        ("198.51.100.1", Documentation),
        ("203.0.113.1", Documentation),
        ("224.0.0.1", Multicast),
        ("239.255.255.255", Multicast),
        ("240.0.0.1", Reserved),
        ("255.255.255.255", Reserved),
        (GLOBAL_V4, Global),
        // IPv6.
        ("::", Unspecified),
        ("::1", Loopback),
        ("100::1", Reserved),
        ("2001::1", Tunnel),
        ("2001:1::1", Reserved),
        ("2001:1::2", Reserved),
        ("2001:2::1", Reserved),
        ("2001:3::1", Reserved),
        ("2001:4:112::1", Reserved),
        ("2001:10::1", Reserved),
        ("2001:20::1", Reserved),
        ("2001:30::1", Reserved),
        ("2001:1ff::1", Reserved),
        ("2001:200::1", Global),
        ("2001:db8::1", Documentation),
        ("2002:a00:1::1", Tunnel),
        ("2620:4f:8000::1", Reserved),
        ("3fff::1", Documentation),
        ("3fff:fff::1", Documentation),
        ("5f00::1", Reserved),
        ("fc00::1", UniqueLocal),
        ("fd00::1", UniqueLocal),
        ("fe80::1", LinkLocal),
        ("febf::1", LinkLocal),
        ("fec0::1", SiteLocal),
        ("ff02::1", Multicast),
        ("4000::1", Reserved),
        ("1fff::1", Reserved),
        ("64:ff9b:1:1::1", Reserved),
        ("::ffff:0:a00:1", Reserved),
        (GLOBAL_V6, Global),
        // Unwrapped forms: IPv4-mapped, IPv4-compatible, NAT64.
        ("::ffff:127.0.0.1", Loopback),
        ("::ffff:10.0.0.1", Private),
        ("::ffff:11.0.0.1", Global),
        ("::10.0.0.1", Private),
        ("::0.0.0.2", Reserved),
        ("64:ff9b::a00:1", Private),
        ("64:ff9b::7f00:1", Loopback),
        ("64:ff9b::b00:1", Global),
        ("64:ff9b:1::a00:1", Private),
    ];
    for (address, class) in rows {
        assert_eq!(classify(ip(address), &[]), class, "{address}");
    }
}

#[test]
fn own_addresses_and_their_links_are_refused() {
    let own = [(ip(GLOBAL_V6_OWN), 64), (ip(GLOBAL_V4_OWN), 24), (ip("192.168.1.17"), 24)];
    assert_eq!(classify(ip(GLOBAL_V6_OWN), &own), Own);
    assert_eq!(classify(ip(GLOBAL_V4_OWN), &own), Own);
    assert_eq!(classify(ip("::ffff:11.0.1.1"), &own), Own);
    assert_eq!(classify(ip("2a00:0:0:2::99"), &own), OnLink);
    assert_eq!(classify(ip("11.0.1.200"), &own), OnLink);
    assert_eq!(classify(ip("192.168.1.5"), &own), OnLink);
    assert_eq!(classify(ip("192.168.1.17"), &own), Own);
    assert_eq!(classify(ip("2a00:0:0:3::1"), &own), Global);
    assert_eq!(classify(ip("11.0.2.1"), &own), Global);
    // A link-local or loopback class stays what it is on a link of the host.
    let own = [(ip("fe80::1"), 64), (ip("127.0.0.1"), 8)];
    assert_eq!(classify(ip("fe80::2"), &own), LinkLocal);
    assert_eq!(classify(ip("127.0.0.2"), &own), Loopback);
    assert_eq!(classify(ip("127.0.0.1"), &own), Own);
}

#[test]
fn strict_passes_only_global_and_private_entries() {
    let strict = Guard::strict();
    let every = [
        Global,
        Loopback,
        Private,
        Shared,
        LinkLocal,
        UniqueLocal,
        SiteLocal,
        Multicast,
        Unspecified,
        Reserved,
        Tunnel,
        Documentation,
        OnLink,
        Own,
    ];
    let plain: Vec<_> = every.into_iter().filter(|class| strict.passes(*class, false)).collect();
    assert_eq!(plain, vec![Global]);
    let private: Vec<_> = every.into_iter().filter(|class| strict.passes(*class, true)).collect();
    assert_eq!(private, vec![Global, Private, Shared, UniqueLocal, OnLink]);
    let loopback = Guard::with_loopback();
    let tests: Vec<_> = every.into_iter().filter(|class| loopback.passes(*class, false)).collect();
    assert_eq!(tests, vec![Global, Loopback]);
}

proptest! {
    #[test]
    fn classify_unwraps_every_mapped_form(bits in any::<u32>()) {
        let v4 = Ipv4Addr::from_bits(bits);
        let plain = classify(IpAddr::V4(v4), &[]);
        let low = |high: [u16; 6]| {
            Ipv6Addr::new(high[0], high[1], high[2], high[3], high[4], high[5], (bits >> 16) as u16, bits as u16)
        };
        prop_assert_eq!(classify(IpAddr::V6(v4.to_ipv6_mapped()), &[]), plain);
        prop_assert_eq!(classify(IpAddr::V6(low([0x64, 0xff9b, 0, 0, 0, 0])), &[]), plain);
        prop_assert_eq!(classify(IpAddr::V6(low([0x64, 0xff9b, 1, 0, 0, 0])), &[]), plain);
        if bits > 1 {
            prop_assert_eq!(classify(IpAddr::V6(v4.to_ipv6_compatible()), &[]), plain);
        }
        // An address of the host stays its own in every form.
        let own = [(IpAddr::V4(v4), 32)];
        prop_assert_eq!(classify(IpAddr::V6(v4.to_ipv6_mapped()), &own), Own);
    }

    #[test]
    fn global_v6_needs_the_global_unicast_block(bits in any::<u128>()) {
        let address = Ipv6Addr::from_bits(bits);
        if classify(IpAddr::V6(address), &[]) == Global {
            prop_assert_eq!(bits >> 125, 1, "{} is global outside 2000::/3", address);
        }
    }
}
