use std::path::Path;

use efr_protocol::{BusKind, ExitKind, ExitSource, Grant};
use rstest::rstest;

use super::{job_refusal, service_socket};
use crate::{Approver, ExitNeed};

fn need(kind: ExitKind, grants: Vec<Grant>) -> ExitNeed {
    let mut need = ExitNeed::new(kind, "x", ExitSource::Predicted);
    need.grants = grants;
    need
}

#[rstest]
#[case::privilege(ExitKind::Privilege)]
#[case::masked_read(ExitKind::MaskedRead)]
#[case::persistence(ExitKind::Persistence)]
#[case::bus(ExitKind::Bus)]
fn kinds_that_no_job_holds_are_refused(#[case] kind: ExitKind) {
    assert!(job_refusal(&need(kind, Vec::new())), "{kind}");
}

#[rstest]
#[case::write(ExitKind::Write)]
#[case::host(ExitKind::Host)]
#[case::host_view(ExitKind::HostView)]
#[case::outside(ExitKind::Outside)]
#[case::upload(ExitKind::Upload)]
#[case::destructive(ExitKind::Destructive)]
#[case::device(ExitKind::Device)]
#[case::above_root(ExitKind::AboveRoot)]
fn other_kinds_may_run_as_a_job(#[case] kind: ExitKind) {
    assert!(!job_refusal(&need(kind, Vec::new())), "{kind}");
}

#[test]
fn a_desktop_form_that_types_is_refused_and_a_read_form_is_not() {
    let mut typing = need(ExitKind::DesktopIpc, Vec::new());
    typing.approver = Approver::User;
    assert!(job_refusal(&typing));
    let mut read = need(ExitKind::DesktopIpc, Vec::new());
    read.approver = Approver::Classifier;
    assert!(!job_refusal(&read));
}

#[test]
fn a_bus_grant_or_a_service_socket_is_refused_whatever_the_kind() {
    let bus = need(ExitKind::Socket, vec![Grant::Bus { bus: BusKind::Session }]);
    assert!(job_refusal(&bus));
    let docker = need(ExitKind::Socket, vec![Grant::Socket { path: "/run/docker.sock".into() }]);
    assert!(job_refusal(&docker));
    let agent =
        need(ExitKind::Socket, vec![Grant::Socket { path: "/run/user/1000/app/app.sock".into() }]);
    assert!(!job_refusal(&agent), "a tool's own socket may serve a job");
}

#[rstest]
#[case("/run/user/1000/systemd/private", true)]
#[case("/run/docker.sock", true)]
#[case("/var/run/docker.sock", true)]
#[case("/run/user/1000/podman/podman.sock", true)]
#[case("/run/containerd/containerd.sock", true)]
#[case("/run/user/1000/docker.sock", true)]
#[case("/run/user/1000/podman.sock", false)]
#[case("/run/user/1000/notdocker.sock", false)]
#[case("/run/user/1000/bus", false)]
fn service_sockets_match_by_whole_components(#[case] path: &str, #[case] refused: bool) {
    assert_eq!(service_socket(Path::new(path)), refused, "{path}");
}
