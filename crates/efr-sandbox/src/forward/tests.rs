use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use pretty_assertions::assert_eq;
use proptest::prelude::{ProptestConfig, any, prop_assert, prop_assert_eq, prop_oneof, proptest};
use proptest::strategy::{Just, Strategy};

use crate::SandboxError;
use crate::forward::{
    BRIDGE_PORT, Change, Debounce, FORWARD_VERSION, FRAME_LEN, Frame, FrameKind, Listen,
    MAX_FORWARD_CONNECTIONS, MAX_REACH_CONNECTIONS, MAX_REPORTED_PORTS, MIN_PORT, PortRefusal,
    REACH_VERSION, SERVICE_PORTS, TCP_LISTEN, TcpRow, loopback_listens, parse_tcp_table, peer_uid,
    refusal, v6_holds,
};
use crate::spec::ForwardPlan;

/// `/proc/self/net/tcp` of a job's network namespace on x86_64 (Linux 7.2), with
/// listens on 127.0.0.1:8765, 127.0.0.2:8768 and 0.0.0.0:8766, a `TIME_WAIT` row and
/// both rows of one connection to 8765.
const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:223D 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 137513865 1 00000000a80eb11f 100 0 0 10 0
   1: 0200007F:2240 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 137513870 1 00000000606ae6a8 100 0 0 10 0
   2: 00000000:223E 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 137513866 1 000000008a82ec33 100 0 0 10 0
   3: 0100007F:223E 0100007F:C634 06 00000000:00000000 03:0000175B 00000000     0        0 0 3 00000000ef5cce66
   4: 0100007F:C70A 0100007F:223D 01 00000000:00000000 00:00000000 00000000  1000        0 137513871 2 000000000641a63e 20 0 0 10 -1
   5: 0100007F:223D 0100007F:C70A 01 00000000:00000000 00:00000000 00000000  1000        0 137513872 1 00000000cc90a0e4 20 0 0 10 -1
";

/// `/proc/self/net/tcp6` of the same namespace: listens on `[::]:5174`, `[::1]:5173` and
/// `[::ffff:127.0.0.1]:8767`, and both rows of one connection to 5173.
const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:1436 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 137513868 1 00000000c43d7974 100 0 0 10 0
   1: 00000000000000000000000001000000:1435 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 137513867 1 00000000e351a09a 100 0 0 10 0
   2: 0000000000000000FFFF00000100007F:223F 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 137513869 1 000000004708ea3c 100 0 0 10 0
   3: 00000000000000000000000001000000:EB2A 00000000000000000000000001000000:1435 01 00000000:00000000 00:00000000 00000000  1000        0 137513875 2 00000000fad4779e 20 0 0 10 -1
   4: 00000000000000000000000001000000:1435 00000000000000000000000001000000:EB2A 01 00000000:00000000 00:00000000 00000000  1000        0 137513876 1 000000009c4a4c83 20 0 0 10 -1
";

fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(a, b, c, d))
}

fn local6() -> IpAddr {
    IpAddr::V6(Ipv6Addr::LOCALHOST)
}

fn listen(addr: IpAddr, port: u16) -> Listen {
    Listen { addr, port }
}

fn row(state: u8, local: SocketAddr, remote: SocketAddr, uid: u32) -> TcpRow {
    TcpRow { state, local, remote, uid }
}

fn addr(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

/// The timings of the tests of 2.0: poll 20 ms, stable 40 ms, ephemeral 200 ms.
fn plan(proxy: bool) -> ForwardPlan {
    ForwardPlan {
        poll_ms: 20,
        stable_ms: 40,
        ephemeral_stable_ms: 200,
        ..ForwardPlan::new("/run/user/1000/efr/sbx/c/jobs/x/forward.sock".into(), proxy)
    }
}

fn debounce() -> Debounce {
    Debounce::new(&plan(false), 32768..=60999)
}

#[test]
fn the_tcp_table_reads_every_row_of_a_real_file() {
    let rows = parse_tcp_table(TCP, false);
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0], row(TCP_LISTEN, addr("127.0.0.1:8765"), addr("0.0.0.0:0"), 1000));
    assert_eq!(rows[1].local, addr("127.0.0.2:8768"));
    assert_eq!(rows[2].local, addr("0.0.0.0:8766"));
    assert_eq!(rows[3], row(0x06, addr("127.0.0.1:8766"), addr("127.0.0.1:50740"), 0));
    assert_eq!(rows[4], row(0x01, addr("127.0.0.1:50954"), addr("127.0.0.1:8765"), 1000));
}

#[test]
fn the_tcp6_table_reads_every_row_of_a_real_file() {
    let rows = parse_tcp_table(TCP6, true);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0], row(TCP_LISTEN, addr("[::]:5174"), addr("[::]:0"), 1000));
    assert_eq!(rows[1].local, addr("[::1]:5173"));
    assert_eq!(rows[2].local, addr("[::ffff:127.0.0.1]:8767"));
    assert_eq!(rows[3], row(0x01, addr("[::1]:60202"), addr("[::1]:5173"), 1000));
}

#[test]
fn a_bad_line_or_the_other_table_is_skipped() {
    let bad = [
        "",
        "   0: 0100007F:223D",
        "   0: 0100007F:223D 00000000:0000 0A 00000000:00000000 00:00000000 00000000  -1",
        "   x: 0100007F:223D 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000",
        "   0: 0100007G:223D 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000",
        "   0: +100007F:223D 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000",
        "   0: 0100007F:23D 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000",
        "   0: 0100007F:223D 00000000:0000 A 00000000:00000000 00:00000000 00000000  1000",
        "   0: 0100007F223D 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000",
    ];
    for line in bad {
        assert_eq!(parse_tcp_table(line, false), [], "{line}");
    }
    // A table of the other family reads as no rows.
    assert_eq!(parse_tcp_table(TCP, true), []);
    assert_eq!(parse_tcp_table(TCP6, false), []);
    // A good line between bad ones still counts.
    let text = format!("garbage\n{}\n\u{0}", TCP.lines().nth(1).unwrap());
    assert_eq!(parse_tcp_table(&text, false).len(), 1);
}

#[test]
fn loopback_listens_map_each_address_and_skip_other_states() {
    let listens = loopback_listens(&parse_tcp_table(TCP, false), &parse_tcp_table(TCP6, true));
    assert_eq!(
        listens,
        [
            listen(local6(), 5173),
            listen(local6(), 5174),
            listen(v4(127, 0, 0, 1), 8765),
            listen(v4(127, 0, 0, 1), 8766),
            listen(v4(127, 0, 0, 1), 8767),
            listen(v4(127, 0, 0, 2), 8768),
        ]
    );
}

#[test]
fn a_listen_that_is_not_on_loopback_is_left_out() {
    let rows = [
        row(TCP_LISTEN, addr("10.0.0.2:8000"), addr("0.0.0.0:0"), 1000),
        row(TCP_LISTEN, addr("[fe80::1]:8001"), addr("[::]:0"), 1000),
        row(TCP_LISTEN, addr("[::ffff:10.0.0.2]:8002"), addr("[::]:0"), 1000),
    ];
    assert_eq!(loopback_listens(&rows[..1], &rows[1..]), []);
}

#[test]
fn one_port_on_v4_and_v6_gives_one_listen_on_v4() {
    let v4_rows = [
        row(TCP_LISTEN, addr("127.0.0.2:8000"), addr("0.0.0.0:0"), 1000),
        row(TCP_LISTEN, addr("127.0.0.1:8000"), addr("0.0.0.0:0"), 1000),
    ];
    let v6_rows = [
        row(TCP_LISTEN, addr("[::1]:8000"), addr("[::]:0"), 1000),
        row(TCP_LISTEN, addr("[::1]:8001"), addr("[::]:0"), 1000),
        row(TCP_LISTEN, addr("127.0.0.3:8001"), addr("0.0.0.0:0"), 1000),
    ];
    assert_eq!(
        loopback_listens(&v4_rows, &v6_rows),
        [listen(v4(127, 0, 0, 1), 8000), listen(v4(127, 0, 0, 3), 8001)]
    );
}

#[test]
fn refusal_names_low_bridge_and_service_ports() {
    let cases = [
        (80, false, Some(PortRefusal::BelowMin)),
        (1023, false, Some(PortRefusal::BelowMin)),
        (1024, false, None),
        (1024, true, None),
        (3128, true, Some(PortRefusal::Bridge)),
        (3128, false, Some(PortRefusal::Service)),
        (1455, false, Some(PortRefusal::Service)),
        (5432, true, Some(PortRefusal::Service)),
        (8765, true, None),
        (35729, false, None),
    ];
    for (port, proxy, expected) in cases {
        assert_eq!(refusal(port, proxy), expected, "{port} {proxy}");
    }
    assert_eq!(PortRefusal::Service.reason(), "a service port");
}

#[test]
fn service_ports_are_sorted_and_hold_the_list_of_oql1() {
    assert!(SERVICE_PORTS.windows(2).all(|pair| pair[0] < pair[1]));
    for port in [1455, 5432, 6379, 2379, 8384, 11434, 27017, 1080, 9050, 9150] {
        assert!(SERVICE_PORTS.contains(&port), "{port}");
    }
    assert!(SERVICE_PORTS.iter().all(|port| *port >= MIN_PORT));
}

#[test]
fn a_port_counts_after_two_polls_at_stable_ms() {
    let mut debounce = debounce();
    let server = [listen(v4(127, 0, 0, 1), 8765)];
    assert_eq!(debounce.observe(0, &server), []);
    assert_eq!(debounce.observe(20, &server), []);
    assert_eq!(debounce.observe(40, &server), [Change::Listen(server[0])]);
    // Reported once.
    assert_eq!(debounce.observe(60, &server), []);
}

#[test]
fn a_port_zero_server_that_lives_one_poll_gives_nothing() {
    let mut debounce = debounce();
    assert_eq!(debounce.observe(0, &[listen(v4(127, 0, 0, 1), 41_234)]), []);
    assert_eq!(debounce.observe(20, &[]), []);
    assert_eq!(debounce.observe(40, &[]), []);
}

#[test]
fn an_ephemeral_port_counts_only_after_ephemeral_stable_ms() {
    let mut debounce = debounce();
    let server = [listen(v4(127, 0, 0, 1), 35729)];
    assert_eq!(debounce.observe(0, &server), []);
    assert_eq!(debounce.observe(40, &server), []);
    assert_eq!(debounce.observe(180, &server), []);
    assert_eq!(debounce.observe(200, &server), [Change::Listen(server[0])]);
}

#[test]
fn a_port_is_gone_after_one_missing_poll_and_counts_again_when_stable() {
    let mut debounce = debounce();
    let server = [listen(v4(127, 0, 0, 1), 8765)];
    debounce.observe(0, &server);
    assert_eq!(debounce.observe(40, &server), [Change::Listen(server[0])]);
    assert_eq!(debounce.observe(60, &[]), [Change::Gone(server[0])]);
    assert_eq!(debounce.observe(80, &server), []);
    assert_eq!(debounce.observe(120, &server), [Change::Listen(server[0])]);
}

#[test]
fn one_port_on_v4_and_v6_gives_one_forward() {
    let mut debounce = debounce();
    let rows4 = [row(TCP_LISTEN, addr("127.0.0.1:5173"), addr("0.0.0.0:0"), 1000)];
    let rows6 = [row(TCP_LISTEN, addr("[::1]:5173"), addr("[::]:0"), 1000)];
    let listens = loopback_listens(&rows4, &rows6);
    debounce.observe(0, &listens);
    assert_eq!(debounce.observe(40, &listens), [Change::Listen(listen(v4(127, 0, 0, 1), 5173))]);
}

#[test]
fn an_address_change_is_gone_then_a_new_listen() {
    let mut debounce = debounce();
    let old = listen(v4(127, 0, 0, 1), 8765);
    let new = listen(local6(), 8765);
    debounce.observe(0, &[old]);
    debounce.observe(40, &[old]);
    assert_eq!(debounce.observe(60, &[new]), [Change::Gone(old)]);
    assert_eq!(debounce.observe(100, &[new]), [Change::Listen(new)]);
}

#[test]
fn low_and_bridge_ports_are_never_reported_and_service_ports_are() {
    let mut proxied = Debounce::new(&plan(true), 32768..=60999);
    let listens = [
        listen(v4(127, 0, 0, 1), 80),
        listen(v4(127, 0, 0, 1), 3128),
        listen(v4(127, 0, 0, 1), 6379),
    ];
    proxied.observe(0, &listens);
    assert_eq!(proxied.observe(40, &listens), [Change::Listen(listens[2])]);
    // Without a proxy, 3128 is a service port, which is reported.
    let mut debounce = debounce();
    debounce.observe(0, &listens);
    assert_eq!(
        debounce.observe(40, &listens),
        [Change::Listen(listens[1]), Change::Listen(listens[2])]
    );
}

/// The ports of `changes`, in order.
fn ports_of(changes: &[Change]) -> Vec<u16> {
    changes
        .iter()
        .map(|change| match change {
            Change::Listen(listen) | Change::Gone(listen) => listen.port,
        })
        .collect()
}

#[test]
fn the_debounce_reports_at_most_the_flood_guard_and_service_ports_do_not_count() {
    let mut debounce = debounce();
    let count = u16::try_from(MAX_REPORTED_PORTS).unwrap() + 4;
    let mut listens: Vec<Listen> =
        (0..count).map(|index| listen(v4(127, 0, 0, 1), 20_000 + index)).collect();
    listens.push(listen(v4(127, 0, 0, 1), 5432));
    debounce.observe(0, &listens);
    let ports = ports_of(&debounce.observe(40, &listens));
    let counted = (0..u16::try_from(MAX_REPORTED_PORTS).unwrap()).map(|index| 20_000 + index);
    assert_eq!(ports, std::iter::once(5432).chain(counted).collect::<Vec<_>>());
    // A counted port that comes back is reported again; a new one is not.
    let back = [listen(v4(127, 0, 0, 1), 20_000), listen(v4(127, 0, 0, 1), 20_000 + count - 1)];
    debounce.observe(60, &[]);
    debounce.observe(80, &back);
    assert_eq!(debounce.observe(120, &back), [Change::Listen(back[0])]);
}

#[test]
fn ports_past_the_jobs_limit_still_report() {
    // efrd may refuse each of the first nine ports, such as for a port in use on the
    // host; the forwarder does not know it, so a tenth port that comes later still
    // reports, and efrd decides.
    let mut debounce = debounce();
    let first: Vec<Listen> = (0..9).map(|index| listen(v4(127, 0, 0, 1), 3000 + index)).collect();
    debounce.observe(0, &first);
    assert_eq!(ports_of(&debounce.observe(40, &first)), (3000..3009).collect::<Vec<_>>());
    let mut all = first.clone();
    all.push(listen(v4(127, 0, 0, 1), 5173));
    debounce.observe(60, &all);
    assert_eq!(debounce.observe(100, &all), [Change::Listen(all[9])]);
}

#[test]
fn time_that_goes_back_counts_as_no_time() {
    let mut debounce = debounce();
    let server = [listen(v4(127, 0, 0, 1), 8765)];
    debounce.observe(1000, &server);
    assert_eq!(debounce.observe(10, &server), []);
}

#[test]
fn peer_uid_takes_the_peer_row() {
    let rows = [
        row(TCP_LISTEN, addr("127.0.0.1:8765"), addr("0.0.0.0:0"), 1000),
        row(0x01, addr("127.0.0.1:50954"), addr("127.0.0.1:8765"), 1001),
        row(0x01, addr("127.0.0.1:8765"), addr("127.0.0.1:50954"), 1000),
    ];
    assert_eq!(peer_uid(&rows, 8765, addr("127.0.0.1:50954")), Some(1001));
    assert_eq!(peer_uid(&rows, 8765, addr("127.0.0.1:50955")), None);
}

#[test]
fn peer_uid_never_takes_efrds_own_row() {
    // Only efrd's own accepted socket is in the table: no peer row, no uid.
    let own = [row(0x01, addr("127.0.0.1:8765"), addr("127.0.0.1:50954"), 1000)];
    assert_eq!(peer_uid(&own, 8765, addr("127.0.0.1:50954")), None);
    // Both rows of one connection: the peer's uid, never efrd's.
    let rows = [own[0], row(0x01, addr("127.0.0.1:50954"), addr("127.0.0.1:8765"), 0)];
    assert_eq!(peer_uid(&rows, 8765, addr("127.0.0.1:50954")), Some(0));
    assert_eq!(peer_uid(&rows, 8765, addr("127.0.0.1:8765")), None);
}

#[test]
fn peer_uid_skips_listeners_and_time_wait_and_refuses_two_uids() {
    let time_wait = row(0x06, addr("127.0.0.1:50954"), addr("127.0.0.1:8765"), 0);
    let peer = row(0x01, addr("127.0.0.1:50954"), addr("127.0.0.1:8765"), 1000);
    assert_eq!(peer_uid(&[time_wait, peer], 8765, addr("127.0.0.1:50954")), Some(1000));
    let other = TcpRow { uid: 1001, ..peer };
    assert_eq!(peer_uid(&[peer, other], 8765, addr("127.0.0.1:50954")), None);
    // A row to another host address than 127.0.0.1 is not the peer of efrd's listener.
    let elsewhere = row(0x01, addr("127.0.0.1:50954"), addr("127.0.0.2:8765"), 1001);
    assert_eq!(peer_uid(&[elsewhere], 8765, addr("127.0.0.1:50954")), None);
}

#[test]
fn peer_uid_takes_only_the_row_of_the_peer_address() {
    // Two connections from the same port of two loopback addresses: each peer gets the
    // uid of its own row.
    let rows = [
        row(0x01, addr("127.0.0.5:50954"), addr("127.0.0.1:8765"), 1001),
        row(0x01, addr("127.0.0.1:50954"), addr("127.0.0.1:8765"), 1000),
    ];
    assert_eq!(peer_uid(&rows, 8765, addr("127.0.0.1:50954")), Some(1000));
    assert_eq!(peer_uid(&rows, 8765, addr("127.0.0.5:50954")), Some(1001));
    assert_eq!(peer_uid(&rows, 8765, addr("127.0.0.6:50954")), None);
    // A peer that is no IPv4 loopback address has no row.
    assert_eq!(peer_uid(&rows, 8765, addr("10.0.0.1:50954")), None);
    assert_eq!(peer_uid(&rows, 8765, addr("[::1]:50954")), None);
}

#[test]
fn peer_uid_reads_a_v4_mapped_row() {
    let rows = parse_tcp_table(
        "   0: 0000000000000000FFFF00000100007F:C70A 0000000000000000FFFF00000100007F:223D 01 \
         00000000:00000000 00:00000000 00000000  1002        0 1 1 0 20 0 0 10 -1",
        true,
    );
    assert_eq!(peer_uid(&rows, 8765, addr("127.0.0.1:50954")), Some(1002));
}

#[test]
fn v6_holds_a_port_on_loopback_or_any_only_while_it_listens() {
    let rows = parse_tcp_table(TCP6, true);
    assert!(v6_holds(&rows, 5173));
    assert!(v6_holds(&rows, 5174));
    // The v4-mapped listen is the IPv4 server's, not a v6 one that shadows it.
    assert!(!v6_holds(&rows, 8767));
    // The other row of the connection holds no port.
    assert!(!v6_holds(&rows, 60202));
    let established = [row(0x01, addr("[::1]:8000"), addr("[::1]:40000"), 1000)];
    assert!(!v6_holds(&established, 8000));
}

#[test]
fn each_frame_has_its_layout() {
    let open = Frame::open(0x0102_0304, listen(v4(127, 0, 0, 1), 8765));
    let mut expected = [0u8; FRAME_LEN];
    expected[..12].copy_from_slice(&[4, 4, 0, 0, 0x22, 0x3D, 0, 0, 1, 2, 3, 4]);
    expected[12..16].copy_from_slice(&[127, 0, 0, 1]);
    assert_eq!(open.encode(), expected);
    let hello = Frame::hello().encode();
    assert_eq!(hello[..4], [1, 4, FORWARD_VERSION, 0]);
    assert!(hello[4..].iter().all(|byte| *byte == 0));
    let gone = Frame::gone(listen(local6(), 5173)).encode();
    assert_eq!(gone[..2], [3, 6]);
    assert_eq!(gone[27], 1);
}

fn refused(bytes: &[u8; FRAME_LEN]) -> &'static str {
    match Frame::decode(bytes) {
        Err(SandboxError::Frame { problem }) => problem,
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn decode_refuses_each_broken_rule() {
    let good = Frame::listen(listen(v4(127, 0, 0, 1), 8765)).encode();
    let with = |index: usize, byte: u8| {
        let mut bytes = good;
        bytes[index] = byte;
        bytes
    };
    assert_eq!(refused(&with(0, 0)), "an unknown kind");
    assert_eq!(refused(&with(0, 10)), "an unknown kind");
    assert_eq!(refused(&with(0, 255)), "an unknown kind");
    assert_eq!(refused(&with(3, 1)), "a reserved byte that is not 0");
    assert_eq!(refused(&with(7, 1)), "a reserved byte that is not 0");
    assert_eq!(refused(&with(20, 1)), "a reserved byte that is not 0");
    assert_eq!(refused(&with(1, 0)), "a family other than 4 and 6");
    assert_eq!(refused(&with(1, 5)), "a family other than 4 and 6");
    assert_eq!(refused(&with(12, 10)), "an address that is not loopback");
    assert_eq!(refused(&with(4, 0)), "a port below 1024");
    assert_eq!(refused(&with(2, 1)), "a status where the kind has none");
    assert_eq!(refused(&with(11, 1)), "an id where the kind has none");
    let mapped = Frame::listen(listen("::ffff:127.0.0.1".parse().unwrap(), 8765)).encode();
    assert_eq!(refused(&mapped), "an address that is not loopback");
    let data = Frame::data(9, 0).encode();
    let mut with_port = data;
    with_port[5] = 1;
    assert_eq!(refused(&with_port), "an address where the kind has none");
    let mut with_addr = data;
    with_addr[12] = 127;
    assert_eq!(refused(&with_addr), "an address where the kind has none");
    let mut hello_id = Frame::hello().encode();
    hello_id[8] = 1;
    assert_eq!(refused(&hello_id), "an id where the kind has none");
}

#[test]
fn each_reach_frame_has_its_layout() {
    let target = listen(v4(127, 0, 0, 1), 8765);
    let reach = Frame::reach(target).encode();
    let mut expected = [0u8; FRAME_LEN];
    expected[..12].copy_from_slice(&[6, 4, 0, 0, 0x22, 0x3D, 0, 0, 0, 0, 0, 0]);
    expected[12..16].copy_from_slice(&[127, 0, 0, 1]);
    assert_eq!(reach, expected);
    assert_eq!(Frame::unreach(target).encode()[0], 7);
    let bound = Frame::bound(listen(local6(), 5173), 98).encode();
    assert_eq!(bound[..4], [8, 6, 98, 0]);
    assert_eq!(bound[27], 1);
    assert_eq!(Frame::connect(target).encode()[0], 9);
    let hello = Frame::reach_hello().encode();
    assert_eq!(hello[..4], [1, 4, REACH_VERSION, 0]);
    assert!(hello[4..].iter().all(|byte| *byte == 0));
    for frame in [Frame::reach(target), Frame::unreach(target), Frame::connect(target)] {
        assert_eq!(Frame::decode(&frame.encode()).unwrap(), frame);
        assert_eq!(frame.target(), target);
    }
    // A reach takes at most half of a job's places.
    assert_eq!(MAX_REACH_CONNECTIONS * 2, MAX_FORWARD_CONNECTIONS);
}

#[test]
fn each_kind_belongs_to_one_connection() {
    let kinds = [
        FrameKind::Hello,
        FrameKind::Listen,
        FrameKind::Gone,
        FrameKind::Open,
        FrameKind::Data,
        FrameKind::Reach,
        FrameKind::Unreach,
        FrameKind::Bound,
        FrameKind::Connect,
    ];
    for kind in kinds {
        let byte = kind as u8;
        assert_eq!(kind.is_reach(), (6..=9).contains(&byte), "{kind:?}");
        assert_eq!(kind.has_listen(), (2..=4).contains(&byte), "{kind:?}");
        assert!(!(kind.is_reach() && kind.has_listen()), "{kind:?}");
        assert_eq!(kind.names_port(), kind.is_reach() || kind.has_listen(), "{kind:?}");
    }
}

#[test]
fn decode_refuses_each_broken_rule_of_a_reach_frame() {
    let target = listen(v4(127, 0, 0, 1), 8765);
    for good in [
        Frame::reach(target),
        Frame::unreach(target),
        Frame::bound(target, 0),
        Frame::connect(target),
    ] {
        let with = |index: usize, byte: u8| {
            let mut bytes = good.encode();
            bytes[index] = byte;
            bytes
        };
        let kind = good.kind;
        // Port 1023, the bridge's port and an address that is not loopback.
        let low = Frame { port: MIN_PORT - 1, ..good }.encode();
        assert_eq!(refused(&low), "a port below 1024", "{kind:?}");
        let bridge = Frame { port: BRIDGE_PORT, ..good }.encode();
        assert_eq!(refused(&bridge), "the port of the proxy bridge", "{kind:?}");
        assert_eq!(refused(&with(12, 10)), "an address that is not loopback", "{kind:?}");
        let mapped = Frame { addr: "::ffff:127.0.0.1".parse().unwrap(), ..good }.encode();
        assert_eq!(refused(&mapped), "an address that is not loopback", "{kind:?}");
        assert_eq!(refused(&with(11, 1)), "an id where the kind has none", "{kind:?}");
        if kind != FrameKind::Bound {
            assert_eq!(refused(&with(2, 1)), "a status where the kind has none", "{kind:?}");
        }
    }
    // A bound carries its errno; port 1024 and `::1` pass.
    let bound = Frame::bound(listen(local6(), MIN_PORT), 98);
    assert_eq!(Frame::decode(&bound.encode()).unwrap(), bound);
}

fn any_reach_listen() -> impl Strategy<Value = Listen> {
    any_listen().prop_filter("the bridge's port", |listen| listen.port != BRIDGE_PORT)
}

fn any_listen() -> impl Strategy<Value = Listen> {
    let addr =
        prop_oneof![any::<[u8; 3]>().prop_map(|[b, c, d]| v4(127, b, c, d)), Just(local6()),];
    (addr, MIN_PORT..=u16::MAX).prop_map(|(addr, port)| Listen { addr, port })
}

fn any_frame() -> impl Strategy<Value = Frame> {
    prop_oneof![
        Just(Frame::hello()),
        any::<u8>().prop_map(|status| Frame { status, ..Frame::hello() }),
        any_listen().prop_map(Frame::listen),
        any_listen().prop_map(Frame::gone),
        (any::<u32>(), any_listen()).prop_map(|(id, target)| Frame::open(id, target)),
        (any::<u32>(), any::<u8>()).prop_map(|(id, status)| Frame::data(id, status)),
        any::<u8>().prop_map(|status| Frame { status, ..Frame::reach_hello() }),
        any_reach_listen().prop_map(Frame::reach),
        any_reach_listen().prop_map(Frame::unreach),
        (any_reach_listen(), any::<u8>()).prop_map(|(target, status)| Frame::bound(target, status)),
        any_reach_listen().prop_map(Frame::connect),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn the_table_parse_never_panics(text in "\\PC*", v6 in any::<bool>()) {
        let _ = parse_tcp_table(&text, v6);
    }

    #[test]
    fn the_table_parse_never_panics_on_near_rows(
        fields in proptest::collection::vec("[0-9A-Fa-f:+ -]{0,40}", 0..12),
        v6 in any::<bool>(),
    ) {
        let line = format!("   0: {}", fields.join(" "));
        let _ = parse_tcp_table(&line, v6);
    }

    #[test]
    fn a_reach_frame_below_1024_or_off_loopback_is_refused(
        kind in prop_oneof![
            Just(FrameKind::Reach),
            Just(FrameKind::Unreach),
            Just(FrameKind::Bound),
            Just(FrameKind::Connect),
        ],
        port in 0..MIN_PORT,
        far in any::<[u8; 4]>().prop_filter("not loopback", |octets| octets[0] != 127),
        good in any_reach_listen(),
    ) {
        let low = Frame { kind, status: 0, id: 0, addr: good.addr, port };
        prop_assert!(Frame::decode(&low.encode()).is_err());
        let off = Frame { kind, status: 0, id: 0, addr: IpAddr::V4(Ipv4Addr::from(far)), port: good.port };
        prop_assert!(Frame::decode(&off.encode()).is_err());
    }

    #[test]
    fn a_frame_decodes_to_what_was_encoded(frame in any_frame()) {
        prop_assert_eq!(Frame::decode(&frame.encode()).unwrap(), frame);
    }

    #[test]
    fn decode_of_any_bytes_never_panics_and_keeps_the_rules(bytes in any::<[u8; FRAME_LEN]>()) {
        if let Ok(frame) = Frame::decode(&bytes) {
            prop_assert_eq!(frame.encode(), bytes);
            prop_assert!(bytes[3] == 0 && bytes[6] == 0 && bytes[7] == 0);
            prop_assert!(bytes[1] == 4 || bytes[1] == 6);
            if frame.kind.names_port() {
                prop_assert!(frame.addr.is_loopback());
                prop_assert!(frame.port >= MIN_PORT);
                if frame.kind.is_reach() {
                    prop_assert!(frame.port != BRIDGE_PORT);
                }
                if frame.kind != FrameKind::Bound {
                    prop_assert_eq!(frame.status, 0);
                }
            } else {
                prop_assert!(matches!(frame.kind, FrameKind::Hello | FrameKind::Data));
                prop_assert_eq!(frame.port, 0);
            }
        }
    }
}
