//! `efr-sbx layers --pid N`: the helper that mounts the cache overlays of one call
//! (`efr_sandbox::CacheLayers`).
//!
//! bwrap cannot pass mount options to an overlay, and without `index=off` and
//! `xino=off` the kernel logs two lines for each overlay of each call and an upper dir
//! can stay busy after a call. So the launcher starts this helper once bwrap's setup is
//! done and the inner stage waits, with `N` the inner stage's process id. The helper
//! enters the call's user namespace (the one that owns the call's mounts) and mount
//! namespace, and for each cache:
//!
//! 1. opens the plan's mounts inside the cache, which bwrap made: masks, pins, floors;
//! 2. mounts the overlay on the cache with `userxattr,index=off,xino=off`, its upper and
//!    work dirs in the staging dir;
//! 3. moves each opened mount onto the overlay, so it still wins over the cache.
//!
//! Then it unmounts the staging dir, so no path inside leads to a layer. It runs
//! before any code of the call, and the call starts only when it exits with 0. Every
//! failure ends it with the reason on stderr, and the call does not run.

use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process::ExitCode;

use efr_sandbox::{CacheLayer, CacheLayers, MAX_LAYERS_BYTES, SETUP_FAILURE_STATUS};
use rustix::fs::{CWD, Mode, OFlags, ResolveFlags};
use rustix::mount::{
    FsMountFlags, FsOpenFlags, MountAttrFlags, MoveMountFlags, OpenTreeFlags, UnmountFlags,
};
use rustix::thread::LinkNameSpaceType;

use crate::error::SbxError;
use crate::fds;

/// Runs the helper for the process `pid`, with the plan on stdin.
pub(crate) fn main(pid: u32) -> ExitCode {
    match run(pid) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "{}", error.chain());
            ExitCode::from(u8::try_from(SETUP_FAILURE_STATUS).unwrap_or(1))
        }
    }
}

fn run(pid: u32) -> Result<(), SbxError> {
    let mut bytes = Vec::new();
    let cap = u64::try_from(MAX_LAYERS_BYTES).unwrap_or(u64::MAX).saturating_add(1);
    std::io::stdin()
        .take(cap)
        .read_to_end(&mut bytes)
        .map_err(|error| SbxError::os("read the cache layers", error))?;
    let layers = CacheLayers::from_json(&bytes)?;
    enter(pid)?;
    for layer in &layers.layers {
        mount(layer, layers.fresh)?;
    }
    rustix::mount::unmount(&layers.staging, UnmountFlags::DETACH)
        .map_err(|error| SbxError::io("unmount", &layers.staging, error.into()))
}

/// Enters the user namespace that owns the mounts of `pid`, then its mount namespace,
/// whose root and working directory become this process's.
fn enter(pid: u32) -> Result<(), SbxError> {
    let open = |kind: &str| {
        let path = format!("/proc/{pid}/ns/{kind}");
        rustix::fs::open(path.as_str(), OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
            .map_err(|error| SbxError::io("open", path, error.into()))
    };
    let user = open("user")?;
    let mounts = open("mnt")?;
    let owner = fds::ns_parent(user.as_fd())
        .map_err(|error| SbxError::os("find the call's first user namespace", error))?;
    rustix::thread::move_into_link_name_space(owner.as_fd(), Some(LinkNameSpaceType::User))
        .map_err(|error| SbxError::os("enter the call's user namespace", error.into()))?;
    rustix::thread::move_into_link_name_space(mounts.as_fd(), Some(LinkNameSpaceType::Mount))
        .map_err(|error| SbxError::os("enter the call's mount namespace", error.into()))
}

/// A directory opened for reading with no symbolic link on the way: a layer of the
/// overlay.
fn layer_dir(path: &Path) -> Result<OwnedFd, SbxError> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    rustix::fs::openat2(CWD, path, flags, Mode::empty(), ResolveFlags::NO_SYMLINKS)
        .map_err(|error| SbxError::io("open", path, error.into()))
}

fn mount(layer: &CacheLayer, fresh: bool) -> Result<(), SbxError> {
    let moved = layer
        .moved
        .iter()
        .map(|path| {
            let flags = OpenTreeFlags::OPEN_TREE_CLOEXEC | OpenTreeFlags::AT_SYMLINK_NOFOLLOW;
            rustix::mount::open_tree(CWD, path, flags)
                .map(|fd| (path, fd))
                .map_err(|error| SbxError::io("open the mount at", path, error.into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if fresh {
        for dir in [&layer.dir, &layer.upper, &layer.work] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(dir)
                .map_err(|error| SbxError::io("create", dir, error))?;
        }
    }
    let lower = layer_dir(&layer.target)?;
    let upper = layer_dir(&layer.upper)?;
    let work = layer_dir(&layer.work)?;
    let target = &layer.target;
    let failed =
        |error: rustix::io::Errno| SbxError::io("mount the overlay on", target, error.into());
    let fs = rustix::mount::fsopen("overlay", FsOpenFlags::FSOPEN_CLOEXEC).map_err(failed)?;
    rustix::mount::fsconfig_set_fd(&fs, "lowerdir+", &lower).map_err(failed)?;
    rustix::mount::fsconfig_set_fd(&fs, "upperdir", &upper).map_err(failed)?;
    rustix::mount::fsconfig_set_fd(&fs, "workdir", &work).map_err(failed)?;
    rustix::mount::fsconfig_set_flag(&fs, "userxattr").map_err(failed)?;
    // NOTE: index=on and xino=auto (the kernel's defaults) need file handles, which a
    // user namespace cannot decode: the kernel falls back and logs it for each mount.
    rustix::mount::fsconfig_set_string(&fs, "index", "off").map_err(failed)?;
    rustix::mount::fsconfig_set_string(&fs, "xino", "off").map_err(failed)?;
    rustix::mount::fsconfig_create(&fs).map_err(failed)?;
    let attrs = MountAttrFlags::MOUNT_ATTR_NOSUID | MountAttrFlags::MOUNT_ATTR_NODEV;
    let overlay =
        rustix::mount::fsmount(&fs, FsMountFlags::FSMOUNT_CLOEXEC, attrs).map_err(failed)?;
    let flags = MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH;
    rustix::mount::move_mount(&overlay, "", CWD, target, flags).map_err(failed)?;
    for (path, fd) in moved {
        rustix::mount::move_mount(&fd, "", CWD, path, flags).map_err(|error| {
            SbxError::io("move the mount onto the overlay at", path, error.into())
        })?;
    }
    Ok(())
}
