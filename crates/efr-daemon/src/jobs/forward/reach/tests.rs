//! The reach of a contained call on efrd's side: the ports and routes of [`Forwards`],
//! and which call gets a plan.

use std::path::Path;
use std::sync::{Arc, Weak};

use efr_config::{JobsSettings, SandboxSettings};
use efr_permissions::Egress;
use efr_protocol::{CacheMode, CallId, ConversationId, Grant, Launch};
use efr_sandbox::{JobPlan, NetworkPlan, SandboxSpec};
use efr_stdx::paths::Dirs;
use pretty_assertions::assert_eq;

use super::plan_of;
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
    forwards.serving(own, Weak::new());
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
    forwards.serving(own, Weak::new());
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
