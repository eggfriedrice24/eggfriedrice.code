//! efrd's side of job port forwards: which job gets forwards, and the forward socket.

use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::path::PathBuf;

use efr_config::JobsSettings;
use efr_permissions::Egress;
use efr_protocol::{BusKind, Grant, Launch};
use efr_sandbox::ForwardPlan;
use pretty_assertions::assert_eq;

use super::{bind, plan_of};

fn socket() -> PathBuf {
    PathBuf::from("/run/user/1000/efr/sbx/c/jobs/k/forward.sock")
}

#[test]
fn contained_job_with_its_own_network_gets_forwards() {
    let on = JobsSettings::default();
    assert!(on.forward_ports, "the default forwards");
    let routine = Launch::contained();
    assert_eq!(
        plan_of(&on, &routine, Egress::None, socket()),
        Some(ForwardPlan::new(socket(), false)),
        "network.mode = none"
    );
    assert_eq!(
        plan_of(&on, &routine, Egress::Proxy, socket()),
        Some(ForwardPlan::new(socket(), true)),
        "the proxy: port 3128 is the bridge's"
    );
    let granted = Launch::Contained {
        grants: vec![
            Grant::Host { host: "gitlab.com".to_owned(), port: 443 },
            Grant::BusFilter { bus: BusKind::Session },
        ],
    };
    assert_eq!(
        plan_of(&on, &granted, Egress::Proxy, socket()),
        Some(ForwardPlan::new(socket(), true)),
        "grants that keep the job's own namespace"
    );
}

#[test]
fn no_forwards_off_in_the_exit_child_or_on_the_host_network() {
    let on = JobsSettings::default();
    let mut off = JobsSettings::default();
    off.forward_ports = false;
    assert_eq!(plan_of(&off, &Launch::contained(), Egress::Proxy, socket()), None, "off");
    for launch in [Launch::Unsandboxed, Launch::Direct] {
        assert_eq!(plan_of(&on, &launch, Egress::Proxy, socket()), None, "{launch:?}");
    }
    for grant in [Grant::OpenNetwork, Grant::NetView] {
        let launch = Launch::Contained { grants: vec![grant.clone()] };
        assert_eq!(plan_of(&on, &launch, Egress::Proxy, socket()), None, "{grant:?}");
        assert_eq!(plan_of(&on, &launch, Egress::None, socket()), None, "{grant:?}");
    }
}

#[tokio::test]
async fn forward_socket_binds_0600_in_a_deep_call_dir() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("r".repeat(60)).join("sbx").join("c".repeat(36)).join("jobs");
    let dir = dir.join("k".repeat(36));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("forward.sock");
    assert!(path.as_os_str().len() > 107, "{}", path.display());
    let socket = bind(&dir, ForwardPlan::new(path.clone(), false)).await.unwrap();
    let meta = std::fs::symlink_metadata(&path).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert!(bind(&dir, ForwardPlan::new(path, false)).await.is_err(), "one socket per job");
    drop(socket);
}
