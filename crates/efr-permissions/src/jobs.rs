//! What a background job may not hold (efr's auto spec, section 3.18, phase C).
//!
//! A job runs on after its turn, while nobody watches it, for up to its time limit. Some
//! exits do not fit that, in every permission mode and whatever the user answers:
//!
//! - `privilege`: a job has no terminal for sudo's password, and with `NOPASSWD` its root
//!   processes outlive `job_stop` and the time limit, because the launcher runs as the
//!   user and cannot signal them.
//! - `masked_read`: no request needs an unmasked secret for hours.
//! - `persistence`: a change of a file that runs code later needs the line at the end of
//!   its turn, which a job that writes after the turn cannot give.
//! - a `desktop_ipc` form that types or starts programs: no request needs a desktop that
//!   types for hours.
//! - the whole message bus (`bus`), and the socket of a service manager or a container
//!   engine ([`SERVICE_SOCKETS`]): through them a job starts a unit or a container that
//!   is not in its process tree, outlives `job_stop` and the time limit, and counts as
//!   the user on efr's socket.
//!
//! The model runs such a part in the foreground and the rest as a job.

use std::path::Path;

use efr_protocol::{ExitKind, Grant};

use crate::{Approver, ExitNeed};

/// What the model reads for a part of a line that cannot run as a job.
pub const JOB_REFUSAL: &str =
    "this cannot run as a job; run that part in the foreground, and the rest as a job";

/// The sockets of service managers and container engines, each as the last components
/// of its path: `docker.sock` matches `/run/docker.sock` and `/run/user/1000/docker.sock`,
/// and `podman/podman.sock` matches `/run/user/1000/podman/podman.sock`.
pub const SERVICE_SOCKETS: &[&str] = &[
    "systemd/private",
    "systemd/io.systemd.Manager",
    "docker.sock",
    "podman/podman.sock",
    "containerd.sock",
    "containerd/containerd.sock",
    "crio.sock",
    "crio/crio.sock",
    "buildkit/buildkitd.sock",
    "lxd/unix.socket",
    "incus/unix.socket",
    "libvirt/libvirt-sock",
    "libvirt/virtqemud-sock",
];

/// True when `need` cannot run as part of a background job, in every mode: see the
/// module's list.
pub fn job_refusal(need: &ExitNeed) -> bool {
    let kind = match need.kind {
        ExitKind::Privilege | ExitKind::MaskedRead | ExitKind::Persistence | ExitKind::Bus => true,
        // NOTE: the engine lets the classifier judge only the read and reload forms of
        // a desktop program; a form that types or starts programs, and a desktop socket
        // that the model names, are the user's.
        ExitKind::DesktopIpc => need.approver != Approver::Classifier,
        _ => false,
    };
    kind || need.grants.iter().any(refused_grant)
}

/// True for a grant that no job holds: the whole bus, or the socket of a service
/// manager or a container engine.
fn refused_grant(grant: &Grant) -> bool {
    match grant {
        Grant::Bus { .. } => true,
        Grant::Socket { path } => service_socket(path),
        _ => false,
    }
}

/// True when `path` ends with the components of one of [`SERVICE_SOCKETS`].
pub fn service_socket(path: &Path) -> bool {
    SERVICE_SOCKETS.iter().any(|socket| path.ends_with(socket))
}

#[cfg(test)]
mod tests;
