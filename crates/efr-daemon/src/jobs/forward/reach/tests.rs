//! The reach of a contained call on efrd's side: the ports and routes of [`Forwards`],
//! which call gets a plan, the ports that a line names and the wait of its fact.

use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::Duration;

use efr_config::{JobsSettings, SandboxSettings};
use efr_permissions::Egress;
use efr_protocol::{CacheMode, CallId, ConversationId, Grant, Launch, Needs};
use efr_sandbox::{JobPlan, NetworkPlan, SandboxSpec};
use efr_stdx::paths::Dirs;
use efr_test_support::{TestClock, Wait};
use pretty_assertions::assert_eq;

use super::{REACH_SETTLE, fact, named_ports, plan_of};
use crate::jobs::JobKey;
use crate::jobs::forward::Forwards;
use crate::sandbox::HostFacts;
use crate::sandbox::plan::{PlanInput, build};
use crate::sandbox::seams::TestHostPorts;

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

fn key(conversation_n: u128, number: u32) -> JobKey {
    JobKey { conversation: conversation(conversation_n), number }
}

fn forwards() -> Forwards {
    Forwards::new(Arc::new(TestHostPorts::new()))
}

/// Marks `port` forwarded for `key`, as a forward whose host bind worked does.
fn forward(forwards: &Forwards, key: JobKey, port: u16) {
    forwards.reserve(key, port).unwrap();
    forwards.forwarded(key, port);
}

#[test]
fn only_bound_ports_of_the_conversation_count() {
    let forwards = forwards();
    let (own, other) = (key(1, 1), key(2, 1));
    forwards.reserve(own, 8765).unwrap();
    assert_eq!(forwards.ports_of(own.conversation), [0u16; 0], "reserved, not bound yet");
    forwards.forwarded(own, 8765);
    forward(&forwards, own, 5173);
    forward(&forwards, other, 9000);
    assert_eq!(forwards.ports_of(own.conversation), [5173, 8765]);
    assert_eq!(forwards.ports_of(other.conversation), [9000]);
    forwards.forwarded(other, 8765);
    assert_eq!(forwards.ports_of(other.conversation), [9000], "only its holder marks it");
    forward(&forwards, own, 3128);
    assert_eq!(forwards.ports_of(own.conversation), [5173, 8765], "never the bridge's port");
    forwards.release(own, 8765);
    assert_eq!(forwards.ports_of(own.conversation), [5173]);
    forwards.release_job(own);
    assert_eq!(forwards.ports_of(own.conversation), [0u16; 0]);
    assert_eq!(forwards.ports_of(other.conversation), [9000]);
}

#[test]
fn a_route_needs_the_job_and_the_conversation() {
    let forwards = forwards();
    let own = key(1, 1);
    forward(&forwards, own, 8765);
    assert!(!forwards.has_forward_job(own.conversation), "no job serves forwards yet");
    forwards.serving(own, Weak::new(), jiff::Timestamp::UNIX_EPOCH);
    assert!(forwards.has_forward_job(own.conversation));
    assert!(!forwards.has_forward_job(conversation(2)));
    assert!(forwards.route(conversation(2), 8765).is_none(), "another conversation");
    assert!(forwards.route(own.conversation, 8766).is_none(), "a port no job forwards");
    assert!(forwards.route(own.conversation, 8765).is_none(), "the job's tasks ended");
    forwards.release_job(own);
    assert!(!forwards.has_forward_job(own.conversation), "the job's end ends its route");
}

#[test]
fn each_forward_and_release_is_a_change() {
    let forwards = forwards();
    let mut changes = forwards.changes();
    let own = key(1, 1);
    forwards.reserve(own, 8765).unwrap();
    assert!(!changes.has_changed().unwrap(), "a reserve is no forward");
    forwards.forwarded(own, 8765);
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    forwards.forwarded(own, 8765);
    assert!(!changes.has_changed().unwrap(), "the same port again");
    forwards.release(own, 8765);
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    forwards.release_job(own);
    assert!(!changes.has_changed().unwrap(), "nothing left to release");
}

#[test]
fn a_line_names_its_loopback_ports() {
    let cases: [(&str, &[u16]); 9] = [
        ("curl -s http://127.0.0.1:8765/x", &[8765]),
        ("curl 'http://localhost:5173' https://example.com:8443", &[5173]),
        ("curl -s \"http://[::1]:9000/\" && echo ok", &[9000]),
        ("curl http://localhost:8765 http://LOCALHOST:8765/a", &[8765]),
        ("curl http://localhost/", &[]),
        ("curl http://127.0.0.1:3128", &[]),
        ("wget http://10.0.0.1:8765", &[]),
        ("echo 127.0.0.1:8765", &[]),
        ("(curl http://127.0.0.1:8080)|head", &[8080]),
    ];
    for (line, ports) in cases {
        assert_eq!(named_ports(line, None), ports, "{line}");
    }
    let needs = Needs {
        hosts: vec!["localhost:8765".to_owned(), "[::1]:8766".to_owned(), "example.com".to_owned()],
        ..Needs::default()
    };
    assert_eq!(named_ports("make check", Some(&needs)), [8765, 8766]);
}

/// The spec of a foreground call of conversation 1 with `launch`, built as efrd builds
/// it, with the proxy when `egress` says so; with `job`, the spec of a job.
fn spec(launch: &Launch, egress: Egress, job: bool) -> SandboxSpec {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let home = root.join("home");
    let dirs = Dirs::new(root.join("c"), root.join("d"), root.join("s"), root.join("r")).unwrap();
    let host = HostFacts { user_runtime: root.join("xrt"), ..HostFacts::default() };
    let settings = SandboxSettings::default();
    let input = PlanInput {
        conversation: conversation(1),
        call: CallId::from_uuid(uuid::Uuid::from_u128(2)),
        launch,
        turn_project: None,
        projects: &[],
        named_paths: &[],
        scratch: &dirs.data().join("scratch/x"),
        settings: &settings,
        secrets: &[],
        protected_config: &[],
        host: &host,
        dirs: &dirs,
        home: &home,
        bwrap: Path::new("/usr/bin/bwrap"),
        cache_mode: CacheMode::Tmp,
        launcher: &dirs.runtime().join("bin/efr-sbx"),
        desktops: &[],
        job: job.then(|| JobPlan::new(60, 19)),
        egress,
    };
    build(&input).spec
}

#[test]
fn only_a_contained_foreground_call_with_its_own_network_gets_a_plan() {
    let forwards = forwards();
    let own = key(1, 1);
    let jobs = JobsSettings::default();
    let contained = spec(&Launch::contained(), Egress::None, false);
    assert_eq!(plan_of(&jobs, &forwards, &contained), None, "no job of the conversation forwards");
    forwards.serving(own, Weak::new(), jiff::Timestamp::UNIX_EPOCH);
    let plan = plan_of(&jobs, &forwards, &contained).expect("a plan with no port yet");
    assert_eq!(
        (plan.socket, plan.ports, plan.proxy),
        (contained.runtime.reach_socket(), vec![], false)
    );
    forward(&forwards, own, 8765);
    let plan = plan_of(&jobs, &forwards, &contained).unwrap();
    assert_eq!(plan.ports, [8765]);
    let mut checked = contained.clone();
    checked.reach = Some(plan);
    checked.check().expect("the launcher's check takes the plan");
    let proxied = spec(&Launch::contained(), Egress::Proxy, false);
    assert!(matches!(proxied.network, NetworkPlan::Proxy { .. }));
    assert!(plan_of(&jobs, &forwards, &proxied).unwrap().proxy);
    let mut off = JobsSettings::default();
    off.forward_ports = false;
    assert_eq!(plan_of(&off, &forwards, &contained), None, "jobs.forward_ports is off");
    for (grants, why) in [
        (vec![Grant::OpenNetwork], "the whole network"),
        (vec![Grant::NetView], "the network view"),
    ] {
        let launch = Launch::Contained { grants };
        assert_eq!(plan_of(&jobs, &forwards, &spec(&launch, Egress::None, false)), None, "{why}");
    }
    assert_eq!(plan_of(&jobs, &forwards, &spec(&Launch::Unsandboxed, Egress::None, false)), None);
    assert_eq!(plan_of(&jobs, &forwards, &spec(&Launch::contained(), Egress::None, true)), None);
}

#[tokio::test]
async fn the_fact_waits_for_the_port_of_a_new_job() {
    let clock = TestClock::new();
    let shared = clock.shared();
    let forwards = forwards();
    let own = key(1, 1);
    forwards.serving(own, Weak::new(), shared.now());
    let waiting = tokio::spawn({
        let (forwards, shared) = (forwards.clone(), Arc::clone(&shared));
        async move { fact(&forwards, &shared, own.conversation, &[8765]).await }
    });
    Wait::new("the fact waits for the settle time")
        .until(|| clock.requested_sleeps().contains(&REACH_SETTLE))
        .await
        .unwrap();
    forward(&forwards, own, 8765);
    assert_eq!(
        waiting.await.unwrap(),
        [8765],
        "the forward ended the wait; the clock did not move"
    );
}

#[tokio::test]
async fn the_fact_waits_at_most_the_rest_of_the_settle_time() {
    let clock = TestClock::new();
    let shared = clock.shared();
    let forwards = forwards();
    let own = key(1, 1);
    forwards.serving(own, Weak::new(), shared.now());
    forward(&forwards, own, 5173);
    clock.advance(Duration::from_secs(1));
    let waiting = tokio::spawn({
        let (forwards, shared) = (forwards.clone(), Arc::clone(&shared));
        async move { fact(&forwards, &shared, own.conversation, &[8765]).await }
    });
    let rest = REACH_SETTLE - Duration::from_secs(1);
    Wait::new("the fact waits for the rest")
        .until(|| clock.requested_sleeps().contains(&rest))
        .await
        .unwrap();
    clock.advance(rest);
    assert_eq!(waiting.await.unwrap(), [5173], "the port never came");
}

#[tokio::test]
async fn the_fact_does_not_wait_without_a_new_job() {
    let clock = TestClock::new();
    let shared = clock.shared();
    let forwards = forwards();
    let own = key(1, 1);
    assert_eq!(fact(&forwards, &shared, own.conversation, &[8765]).await, [0u16; 0], "no job");
    forwards.serving(own, Weak::new(), shared.now());
    forward(&forwards, own, 8765);
    assert_eq!(fact(&forwards, &shared, own.conversation, &[8765]).await, [8765], "forwarded");
    assert_eq!(fact(&forwards, &shared, own.conversation, &[]).await, [8765], "no port named");
    clock.advance(REACH_SETTLE);
    assert_eq!(fact(&forwards, &shared, own.conversation, &[9000]).await, [8765], "an old job");
    assert!(clock.requested_sleeps().is_empty(), "{:?}", clock.requested_sleeps());
}
