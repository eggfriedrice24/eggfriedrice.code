//! The floor checks of an always-allow rule when it loads (efr's auto spec 11.3, DE23 to
//! DE25): what a rule may never name, on the text of its path. The matcher checks the
//! real paths of each call again (`cover.rs`).

use std::path::{Path, PathBuf};

use super::{AllowFloor, AllowRule};
use crate::exits::{Envelope, is_credential_agent, named_desktop};
use crate::jobs::service_socket;
use crate::network::{HostList, HostVerdict};
use crate::path_class::normalize;
use crate::{Locations, PermissionsError, persistence_floors, sandbox_masks};

/// Checks `rule` against the floors of the machine that `locations` and `hosts`
/// describe: the shape first, then each floor of its kind. The first floor that it
/// crosses is the error.
pub(crate) fn check(
    rule: &AllowRule,
    locations: &Locations,
    hosts: &HostList,
) -> Result<(), AllowFloor> {
    if let Err(PermissionsError::AllowRule { problem, .. }) = rule.check_shape(0) {
        return Err(AllowFloor::BadShape { problem });
    }
    match rule {
        AllowRule::Host { host, port } => match hosts.decide(host, *port) {
            HostVerdict::Denied => Err(AllowFloor::NetworkDeny),
            _ => Ok(()),
        },
        AllowRule::Write { under, .. } => {
            write(&rule_path(under, locations).ok_or(AllowFloor::OutsideHome)?, locations)
        }
        AllowRule::Socket { path } => {
            socket(&rule_path(path, locations).ok_or(AllowFloor::OutsideHome)?, locations)
        }
    }
}

/// The path of a rule in normal form, `~` read as the home directory, under the home
/// directory when it lies under an alias.
pub(crate) fn rule_path(path: &str, locations: &Locations) -> Option<PathBuf> {
    let home = locations.home();
    let path = match path.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) => home.join(rest.strip_prefix('/')?),
        None => PathBuf::from(path),
    };
    let path = normalize(&path)?;
    Some(locations.rehome(&path).into_owned())
}

/// The checks of a `write` rule whose dir is `under` (DE24): first what it lies in,
/// then what it holds.
fn write(under: &Path, locations: &Locations) -> Result<(), AllowFloor> {
    let home = locations.home();
    if locations.is_at_or_above_home(under) {
        return Err(AllowFloor::AtOrAboveHome);
    }
    if !under.starts_with(home) {
        return Err(AllowFloor::OutsideHome);
    }
    guarded(under, locations)?;
    if git_dir(under) {
        return Err(AllowFloor::GitDir);
    }
    if let Some(floor) = floor_at_or_above(under, locations) {
        return Err(AllowFloor::UnderFloor { floor });
    }
    if let Some(floor) = persistence_floors(home).into_iter().find(|floor| floor.starts_with(under))
    {
        return Err(AllowFloor::AboveHomeFloor { floor });
    }
    if locations.write_sealed_below(under).is_some() {
        return Err(AllowFloor::Config);
    }
    if locations.efr_paths().any(|root| root.starts_with(under)) {
        return Err(AllowFloor::EfrDir);
    }
    if !locations.secrets_below(under).is_empty() {
        return Err(AllowFloor::Secret);
    }
    if locations.synced_roots().iter().any(|synced| synced.starts_with(under)) {
        return Err(AllowFloor::Synced);
    }
    if let Some(root) = locations.envelope_roots().find(|root| root.starts_with(under)) {
        return Err(AllowFloor::WriteRoot { root: root.to_path_buf() });
    }
    if locations.projects_below(under).len() > 1 {
        return Err(AllowFloor::ManyProjects);
    }
    Ok(())
}

/// The checks of a `socket` rule whose socket is `path` (DE25).
fn socket(path: &Path, locations: &Locations) -> Result<(), AllowFloor> {
    guarded(path, locations)?;
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if service_socket(path)
        || matches!(name, "docker.sock" | "podman.sock" | "containerd.sock")
        || below_service_manager(path)
    {
        return Err(AllowFloor::ServiceSocket);
    }
    if is_bus_socket(path, name) {
        return Err(AllowFloor::BusSocket);
    }
    if is_credential_agent(path, locations) {
        return Err(AllowFloor::CredentialAgent);
    }
    if named_desktop(path, name).is_some() {
        return Err(AllowFloor::DesktopSocket);
    }
    if real_write_roots(locations).iter().any(|root| path.starts_with(root)) {
        return Err(AllowFloor::InWriteRoot);
    }
    Ok(())
}

/// The floors that every kind of rule respects, for `path` and what lies at or above
/// it: a secret, efr's config, efr's own dirs, a mask and a synced folder.
fn guarded(path: &Path, locations: &Locations) -> Result<(), AllowFloor> {
    if locations.is_sealed(path) || locations.is_secret(path) {
        return Err(AllowFloor::Secret);
    }
    if locations.is_write_sealed(path) {
        return Err(AllowFloor::Config);
    }
    if locations.is_efr_own(path) || locations.efr_paths().any(|root| path.starts_with(root)) {
        return Err(AllowFloor::EfrDir);
    }
    let home = locations.home();
    if sandbox_masks(home).iter().chain(locations.mask_roots()).any(|mask| path.starts_with(mask)) {
        return Err(AllowFloor::Mask);
    }
    if locations.synced_roots().iter().any(|synced| path.starts_with(synced)) {
        return Err(AllowFloor::Synced);
    }
    Ok(())
}

/// The floor that `path` is or lies below: a built-in floor of the home directory, a
/// floor that the daemon added, or a protected name in a write root (`Envelope`).
fn floor_at_or_above(path: &Path, locations: &Locations) -> Option<PathBuf> {
    let home = locations.home();
    let found = persistence_floors(home)
        .into_iter()
        .chain(locations.floor_roots().cloned())
        .find(|floor| path.starts_with(floor));
    found.or_else(|| Envelope::new(locations, None, None).is_floor(path).then(|| path.into()))
}

/// True when `path` has a `.git` part.
pub(crate) fn git_dir(path: &Path) -> bool {
    path.components().any(|part| part.as_os_str() == ".git")
}

/// True when `path` lies below the dir of a service manager's own sockets:
/// `/run/systemd/` for the system, `/run/user/<uid>/systemd/` for the user.
pub(crate) fn below_service_manager(path: &Path) -> bool {
    if path.starts_with("/run/systemd") {
        return true;
    }
    let Ok(rest) = path.strip_prefix("/run/user") else { return false };
    let mut parts = rest.components();
    parts.next().is_some() && parts.next().is_some_and(|part| part.as_os_str() == "systemd")
}

/// True when `path`, whose file name is `name`, is the system bus or a session bus.
pub(crate) fn is_bus_socket(path: &Path, name: &str) -> bool {
    path.starts_with("/run/dbus")
        || path.starts_with("/var/run/dbus")
        || (name == "bus" && path.starts_with("/run/user"))
}

/// The write roots whose writes reach the real files: every envelope root but the
/// private tmp and the cache overlays, and every registered project.
pub(crate) fn real_write_roots(locations: &Locations) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = locations
        .envelope_roots()
        .filter(|root| !locations.is_private_root(root))
        .map(Path::to_path_buf)
        .collect();
    for project in locations.projects_below(Path::new("/")) {
        if !locations.is_at_or_above_home(project) && !roots.iter().any(|root| root == project) {
            roots.push(project.to_path_buf());
        }
    }
    roots
}
