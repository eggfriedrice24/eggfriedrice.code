use super::{Graph, check};

fn graph(members: &[&str], normal: &[(&str, &str)], dev: &[(&str, &str)]) -> Graph {
    let mut graph =
        Graph { members: members.iter().map(|m| (*m).to_owned()).collect(), ..Graph::default() };
    for (from, to) in normal {
        graph.add_normal(from, to);
    }
    for (from, to) in dev {
        graph.add_dev(from, to);
    }
    graph
}

#[test]
fn xtask_alone_is_clean() {
    assert!(check(&graph(&["xtask"], &[("xtask", "anyhow")], &[])).is_empty());
}

#[test]
fn allowed_edge_is_clean() {
    let g = graph(&["efr-stdx", "efr-protocol"], &[("efr-protocol", "efr-stdx")], &[]);
    assert!(check(&g).is_empty());
}

#[test]
fn edge_not_on_allowlist_fails() {
    let g = graph(&["efr-stdx", "efr-protocol"], &[("efr-stdx", "efr-protocol")], &[]);
    let violations = check(&g);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("efr-stdx -> efr-protocol"));
}

#[test]
fn unknown_member_fails() {
    let violations = check(&graph(&["efr-mystery"], &[], &[]));
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("efr-mystery"));
}

#[test]
fn third_party_dependencies_are_not_allowlisted() {
    let g = graph(&["efr-stdx"], &[("efr-stdx", "jiff")], &[]);
    assert!(check(&g).is_empty());
}

#[test]
fn forbidden_edge_is_found_transitively() {
    // efr-protocol -> serde-thing -> tokio: the protocol crate must stay runtime free
    // even through a third-party crate.
    let g = graph(
        &["efr-stdx", "efr-protocol"],
        &[("efr-protocol", "efr-stdx"), ("efr-protocol", "serde-thing"), ("serde-thing", "tokio")],
        &[],
    );
    let violations = check(&g);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("efr-protocol -> serde-thing -> tokio"));
}

#[test]
fn exclusive_crate_through_its_owner_is_clean() {
    let g = graph(
        &["efr-stdx", "efr-protocol", "efr-store", "efr-conversation"],
        &[
            ("efr-protocol", "efr-stdx"),
            ("efr-store", "efr-protocol"),
            ("efr-store", "rusqlite"),
            ("efr-conversation", "efr-store"),
        ],
        &[],
    );
    assert!(check(&g).is_empty());
}

#[test]
fn exclusive_crate_outside_its_owner_fails() {
    let g = graph(&["efr-scope"], &[("efr-scope", "rusqlite")], &[]);
    let violations = check(&g);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("only efr-store may depend on rusqlite"));
}

#[test]
fn ghostty_outside_the_ghostty_screen_fails() {
    let g = graph(&["efr-screen-vt100"], &[("efr-screen-vt100", "libghostty-vt")], &[]);
    assert_eq!(check(&g).len(), 1);
}

#[test]
fn test_daemon_as_dev_dependency_of_daemon_and_cli_is_clean() {
    let g = graph(
        &["efr-daemon", "efr-cli", "efr-test-daemon"],
        &[("efr-test-daemon", "efr-daemon")],
        &[("efr-daemon", "efr-test-daemon"), ("efr-cli", "efr-test-daemon")],
    );
    assert!(check(&g).is_empty());
}

#[test]
fn test_daemon_as_dev_dependency_elsewhere_fails() {
    let g = graph(&["efr-store", "efr-test-daemon"], &[], &[("efr-store", "efr-test-daemon")]);
    let violations = check(&g);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("efr-store has efr-test-daemon"));
}

#[test]
fn daemon_as_direct_dev_dependency_fails() {
    let g = graph(&["efr-cli", "efr-daemon"], &[], &[("efr-cli", "efr-daemon")]);
    assert_eq!(check(&g).len(), 1);
}
