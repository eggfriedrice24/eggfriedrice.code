//! A contained call reaches the forwarded ports of its conversation's jobs on loopback
//! with no exit (v0.0.4 shell items, DS43): the fact `CallFacts::reach_ports`, under
//! efr's proxy and with no proxy.

use std::sync::Arc;

use efr_protocol::{ExitKind, Grant, Needs};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::auto::auto_locations;
use super::proxy::{grants, host_list, in_auto, proxy_engine, shell};
use crate::{CallFacts, Decision, Engine, Requirements};

/// The engine of the auto tests with no proxy (`network.mode = "none"`).
fn none_engine() -> Engine {
    Engine::with_defaults(auto_locations()).with_hosts(Arc::new(host_list(&[], &[])))
}

/// The shell call of `line`, with the network flag of the shell tool, whose
/// conversation forwards `ports` for its jobs.
fn reaching(line: &str, ports: &[u16]) -> Requirements {
    let facts = CallFacts { reach_ports: ports.to_vec(), ..CallFacts::default() };
    shell(line).with_network().with_facts(facts)
}

fn kinds(decision: &Decision) -> Vec<ExitKind> {
    decision.exits().map(|need| need.kind).collect()
}

#[rstest]
#[case::loopback("curl http://127.0.0.1:8765/")]
#[case::localhost("curl -s http://localhost:8765/health")]
#[case::v6("curl 'http://[::1]:8765/'")]
#[case::two("curl http://127.0.0.1:8765/a http://localhost:8765/b")]
fn a_forwarded_port_needs_no_exit(#[case] line: &str) {
    for engine in [proxy_engine(), none_engine()] {
        let decision = engine.decide(&in_auto(reaching(line, &[8765])));
        assert_eq!(kinds(&decision), [], "{line}: {:?}", decision.exits().collect::<Vec<_>>());
    }
}

#[test]
fn another_port_keeps_the_whole_network() {
    for engine in [proxy_engine(), none_engine()] {
        let decision = engine.decide(&in_auto(reaching("curl http://127.0.0.1:8766/", &[8765])));
        assert_eq!(grants(&decision), [Grant::OpenNetwork]);
    }
}

#[test]
fn no_fact_gives_todays_exits() {
    for engine in [proxy_engine(), none_engine()] {
        let line = "curl http://127.0.0.1:8765/";
        let without = engine.decide(&in_auto(shell(line).with_network()));
        let empty = engine.decide(&in_auto(reaching(line, &[])));
        assert_eq!(grants(&without), [Grant::OpenNetwork]);
        assert_eq!(grants(&empty), grants(&without));
    }
}

#[test]
fn a_forwarded_port_and_another_host_grant_only_the_other_host() {
    let line = "curl http://localhost:8765 https://example.com";
    let decision = proxy_engine().decide(&in_auto(reaching(line, &[8765])));
    assert_eq!(grants(&decision), [Grant::Host { host: "example.com".to_owned(), port: 443 }]);
    // With no proxy one URL that leaves opens the whole network, as today.
    let decision = none_engine().decide(&in_auto(reaching(line, &[8765])));
    assert_eq!(grants(&decision), [Grant::OpenNetwork]);
}

#[test]
fn the_network_flag_makes_no_second_exit() {
    // The flag of the shell tool alone, with no proxy, opens the whole network for a
    // line that names no URL; a line whose every URL is a forwarded port needs none.
    let decision =
        none_engine().decide(&in_auto(reaching("wget -qO- http://[::1]:8765/", &[8765])));
    assert_eq!(kinds(&decision), []);
    let decision = none_engine().decide(&in_auto(reaching("nc -z localhost 8765", &[8765])));
    assert_eq!(grants(&decision), [Grant::OpenNetwork], "a line with no URL keeps the flag");
}

#[test]
fn needs_hosts_of_a_forwarded_port_make_no_exit_and_a_note() {
    for engine in [proxy_engine(), none_engine()] {
        for word in ["localhost:8765", "127.0.0.1:8765", "[::1]:8765"] {
            let needs = Needs { hosts: vec![word.to_owned()], ..Needs::default() };
            let facts = CallFacts { reach_ports: vec![8765], ..CallFacts::default() };
            let decision =
                engine.decide(&in_auto(shell("make").with_needs(needs).with_facts(facts)));
            assert_eq!(kinds(&decision), [], "{word}");
            assert_eq!(decision.notes().len(), 1, "{word}: {:?}", decision.notes());
            assert!(
                decision.notes()[0].contains(
                    "is a forwarded port of a job of this conversation; a contained call \
                     reaches it on 127.0.0.1 with no needs.hosts entry."
                ),
                "{:?}",
                decision.notes()
            );
        }
    }
    // Another port still asks for the whole network.
    let needs = Needs { hosts: vec!["localhost:8766".to_owned()], ..Needs::default() };
    let facts = CallFacts { reach_ports: vec![8765], ..CallFacts::default() };
    let decision =
        proxy_engine().decide(&in_auto(shell("make").with_needs(needs).with_facts(facts)));
    assert_eq!(grants(&decision), [Grant::OpenNetwork]);
}
