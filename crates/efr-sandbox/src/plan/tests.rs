use std::path::{Path, PathBuf};

use efr_protocol::{BusKind, CacheMode, Grant, SandboxPathRole};
use pretty_assertions::assert_eq;

use crate::paths::depth;
use crate::plan::{MountOp, MountOrigin, MountPlan, OpKind, PlanNote, StartDir};
use crate::spec::{Floor, FloorKind, Mask, MaskKind, WriteRoot, WriteRootKind};
use crate::state::SandboxCwd;
use crate::testing::{FakeFs, PROJECT, spec, world};
use crate::{SandboxError, SandboxSpec};

fn plan(spec: &SandboxSpec, fs: &FakeFs) -> MountPlan {
    MountPlan::build(spec, fs).unwrap()
}

/// The read-only binds of `plan` whose origin is a floor, by target.
fn floors(plan: &MountPlan) -> Vec<(PathBuf, FloorKind)> {
    plan.mounts()
        .iter()
        .filter_map(|mount| match (&mount.op, mount.origin) {
            (MountOp::Bind { target, writable: false, .. }, MountOrigin::Floor(kind)) => {
                Some((target.clone(), kind))
            }
            _ => None,
        })
        .collect()
}

fn has_mount(plan: &MountPlan, op: &MountOp) -> bool {
    plan.mounts().iter().any(|mount| mount.op == *op)
}

fn ro(path: &str) -> MountOp {
    MountOp::Bind { source: path.into(), target: path.into(), writable: false }
}

fn rw(path: &str) -> MountOp {
    MountOp::Bind { source: path.into(), target: path.into(), writable: true }
}

#[test]
fn plan_orders_mounts_by_depth_then_kind() {
    let mut spec = spec();
    // Four kinds at the depth of the project's children: a mask, a write root of the
    // user, a widening and a floor.
    spec.masks.push(Mask { path: "/home/u/p/app/.env".into(), kind: MaskKind::ProjectEnv });
    spec.write_roots
        .push(WriteRoot { path: "/home/u/p/lib".into(), kind: WriteRootKind::NamedProject });
    spec.grants.push(Grant::Write { path: "/home/u/p/out".into() });
    spec.floors.push(Floor { path: "/home/u/p/app/.envrc".into(), kind: FloorKind::ProtectedName });
    let mut fs = world();
    fs.file("/home/u/p/app/.env", "KEY=1")
        .dir("/home/u/p/lib")
        .dir("/home/u/p/out")
        .file("/home/u/p/app/.envrc", "");
    let plan = plan(&spec, &fs);

    let keys: Vec<(usize, OpKind)> =
        plan.mounts().iter().map(|mount| (depth(mount.op.target()), mount.kind)).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "{:#?}", plan.mounts());

    let projects: Vec<OpKind> = plan
        .mounts()
        .iter()
        .filter(|mount| depth(mount.op.target()) == 4 && mount.op.target().starts_with("/home/u/p"))
        .map(|mount| mount.kind)
        .collect();
    assert_eq!(projects, [OpKind::WriteRoot, OpKind::WriteRoot, OpKind::Widening]);
    let at_depth_four: Vec<(&Path, OpKind)> = plan
        .mounts()
        .iter()
        .filter(|mount| mount.op.target().parent() == Some(Path::new(PROJECT)))
        .map(|mount| (mount.op.target(), mount.kind))
        .collect();
    assert_eq!(
        at_depth_four,
        [
            (Path::new("/home/u/p/app/.env"), OpKind::Mask),
            (Path::new("/home/u/p/app/.git"), OpKind::WriteRoot),
            (Path::new("/home/u/p/app/.envrc"), OpKind::Floor),
        ]
    );
}

#[test]
fn plan_rejects_write_root_under_mask() {
    let mut spec = spec();
    spec.write_roots =
        vec![WriteRoot { path: "/home/u/.ssh/keys".into(), kind: WriteRootKind::TurnProject }];
    let mut fs = world();
    fs.dir("/home/u/.ssh/keys");
    let error = MountPlan::build(&spec, &fs).unwrap_err();
    assert!(
        matches!(&error, SandboxError::RootUnderMask { mask, .. } if mask == Path::new("/home/u/.ssh")),
        "{error:?}"
    );
}

#[test]
fn plan_rejects_widening_inside_floor() {
    let mut spec = spec();
    spec.floors.push(Floor { path: "/home/u/.config/systemd".into(), kind: FloorKind::Autostart });
    spec.grants.push(Grant::Write { path: "/home/u/.config/systemd/user/x.service".into() });
    let mut fs = world();
    fs.file("/home/u/.config/systemd/user/x.service", "");
    let error = MountPlan::build(&spec, &fs).unwrap_err();
    assert!(matches!(error, SandboxError::RootUnderFloor { .. }), "{error:?}");
}

#[test]
fn plan_rejects_symlinked_pin() {
    let mut fs = world();
    fs.remove("/home/u/p/app/.git")
        .dir("/home/u/.config/systemd")
        .link("/home/u/p/app/.git", "/home/u/.config/systemd");
    let error = MountPlan::build(&spec(), &fs).unwrap_err();
    assert!(matches!(error, SandboxError::SymlinkedPin { .. }), "{error:?}");

    let mut fs = world();
    fs.remove("/home/u/p/app/.git/hooks")
        .dir("/home/u/hooks")
        .link("/home/u/p/app/.git/hooks", "/home/u/hooks");
    let error = MountPlan::build(&spec(), &fs).unwrap_err();
    assert!(matches!(error, SandboxError::SymlinkedFloor { .. }), "{error:?}");
}

#[test]
fn plan_skips_missing_floor() {
    let mut spec = spec();
    spec.floors
        .push(Floor { path: "/home/u/p/app/.claude".into(), kind: FloorKind::ProtectedName });
    let plan = plan(&spec, &world());
    assert!(!floors(&plan).iter().any(|(path, _)| path == Path::new("/home/u/p/app/.claude")));
    assert!(plan.notes().contains(&PlanNote::MissingFloor("/home/u/p/app/.claude".into())));
}

#[test]
fn plan_binds_config_link_targets_ro() {
    let mut spec = spec();
    spec.write_roots =
        vec![WriteRoot { path: "/home/u/dotfiles".into(), kind: WriteRootKind::TurnProject }];
    spec.floors
        .push(Floor { path: "/home/u/.config/efr/config.toml".into(), kind: FloorKind::Config });
    let mut fs = world();
    fs.remove("/home/u/.config/efr/config.toml")
        .file("/home/u/dotfiles/efr/config.toml", "")
        .link("/home/u/.config/efr/config.toml", "../../dotfiles/efr/config.toml");
    let plan = plan(&spec, &fs);
    assert!(
        floors(&plan).contains(&("/home/u/dotfiles/efr/config.toml".into(), FloorKind::Config))
    );
    assert!(has_mount(&plan, &ro("/home/u/dotfiles/efr/config.toml")));
    // The config root itself lies in no write root: it is read-only already.
    assert!(!floors(&plan).iter().any(|(path, _)| path == Path::new("/home/u/.config/efr")));
}

#[test]
fn plan_binds_dotfile_link_targets_ro() {
    let mut spec = spec();
    spec.write_roots =
        vec![WriteRoot { path: "/home/u/dotfiles".into(), kind: WriteRootKind::TurnProject }];
    let mut fs = world();
    fs.remove("/home/u/.zshrc")
        .file("/home/u/dotfiles/zsh/.zshrc", "")
        .link("/home/u/.zshrc", "dotfiles/zsh/.zshrc");
    let plan = plan(&spec, &fs);
    assert!(
        floors(&plan).contains(&("/home/u/dotfiles/zsh/.zshrc".into(), FloorKind::ShellStartup))
    );
}

#[test]
fn plan_floors_path_dirs_in_roots() {
    let mut spec = spec();
    spec.shell_path =
        "/usr/bin:/home/u/p/app/bin:/home/u/p/app/missing:/home/u/.local/bin".to_owned();
    let mut fs = world();
    fs.dir("/home/u/p/app/bin").dir("/home/u/.local/bin");
    let plan = plan(&spec, &fs);
    let path_dirs: Vec<PathBuf> = floors(&plan)
        .into_iter()
        .filter(|(_, kind)| *kind == FloorKind::PathDir)
        .map(|(path, _)| path)
        .collect();
    assert_eq!(path_dirs, [PathBuf::from("/home/u/p/app/bin")]);
}

#[test]
fn plan_refuses_relative_path_entry() {
    for path in ["bin:/usr/bin", "/usr/bin::/bin", "/usr/bin:", "./node_modules/.bin"] {
        let mut spec = spec();
        spec.shell_path = path.to_owned();
        let error = MountPlan::build(&spec, &world()).unwrap_err();
        assert!(matches!(error, SandboxError::RelativePathEntry { .. }), "{path}: {error:?}");
    }
}

#[test]
fn plan_masks_other_scratch_dirs() {
    let spec = spec();
    let mut fs = world();
    fs.dir("/home/u/.local/share/efr/scratch/other")
        .file("/home/u/.local/share/efr/efr.sqlite", "");
    let plan = plan(&spec, &fs);
    assert!(has_mount(
        &plan,
        &MountOp::Tmpfs { target: "/home/u/.local/share/efr".into(), perms: 0o500 }
    ));
    assert!(has_mount(&plan, &rw(&spec.runtime.scratch.to_string_lossy())));
    let other = plan.explain(Path::new("/home/u/.local/share/efr/scratch/other"));
    assert_eq!(other.role, SandboxPathRole::Masked);
    assert_eq!(
        plan.explain(Path::new("/home/u/.local/share/efr/efr.sqlite")).role,
        SandboxPathRole::Masked
    );
    let own = plan.explain(&spec.runtime.scratch.join("notes.md"));
    assert_eq!(own.role, SandboxPathRole::WriteRoot);
    assert!(own.write);
}

#[test]
fn plan_mask_beats_floor_on_same_path() {
    let mut spec = spec();
    spec.write_roots =
        vec![WriteRoot { path: "/home/u/dotfiles".into(), kind: WriteRootKind::TurnProject }];
    spec.floors.push(Floor { path: "/home/u/.npmrc".into(), kind: FloorKind::ToolConfig });
    let mut fs = world();
    fs.file("/home/u/dotfiles/npmrc", "//registry/:_authToken=x")
        .link("/home/u/.npmrc", "dotfiles/npmrc");
    let plan = plan(&spec, &fs);
    assert!(has_mount(&plan, &MountOp::DevNull { target: "/home/u/dotfiles/npmrc".into() }));
    assert!(!has_mount(&plan, &ro("/home/u/dotfiles/npmrc")), "{:#?}", plan.mounts());
    assert!(plan.notes().iter().any(|note| matches!(note, PlanNote::FloorUnderMask { .. })));
    assert_eq!(plan.explain(Path::new("/home/u/dotfiles/npmrc")).role, SandboxPathRole::Masked);
}

#[test]
fn plan_masks_secret_link_targets() {
    let mut fs = world();
    fs.file("/home/u/dotfiles/npmrc", "token").link("/home/u/.npmrc", "/home/u/dotfiles/npmrc");
    let plan = plan(&spec(), &fs);
    assert!(has_mount(&plan, &MountOp::DevNull { target: "/home/u/dotfiles/npmrc".into() }));
    assert!(has_mount(&plan, &MountOp::Tmpfs { target: "/home/u/.ssh".into(), perms: 0o500 }));
    assert!(!plan.explain(Path::new("/home/u/.ssh/id_ed25519")).read);
}

#[test]
fn plan_refuses_home_as_write_root() {
    for root in ["/home/u", "/home", "/", "/home/u/p/.."] {
        let mut spec = spec();
        let path = crate::paths::normalize(Path::new(root)).unwrap();
        spec.write_roots = vec![WriteRoot { path, kind: WriteRootKind::TurnProject }];
        let error = MountPlan::build(&spec, &world()).unwrap_err();
        assert!(matches!(error, SandboxError::RootTooWide { .. }), "{root}: {error:?}");
    }
    let mut spec = spec();
    spec.grants.push(Grant::Write { path: "/home/u".into() });
    assert!(matches!(MountPlan::build(&spec, &world()), Err(SandboxError::RootTooWide { .. })));
    // A link to the home directory is the home directory.
    let mut spec = crate::testing::spec();
    spec.write_roots =
        vec![WriteRoot { path: "/srv/home".into(), kind: WriteRootKind::UserConfigured }];
    let mut fs = world();
    fs.link("/srv/home", "/home/u");
    assert!(matches!(MountPlan::build(&spec, &fs), Err(SandboxError::RootTooWide { .. })));
}

#[test]
fn plan_pins_only_top_git_dirs() {
    let mut fs = world();
    fs.dir("/home/u/p/app/sub/.git/hooks").file("/home/u/p/app/sub/.git/config", "");
    let plan = plan(&spec(), &fs);
    let pins: Vec<&Path> = plan
        .mounts()
        .iter()
        .filter(|mount| mount.origin == MountOrigin::Pin)
        .map(|mount| mount.op.target())
        .collect();
    assert_eq!(pins, [Path::new("/home/u/p/app/.git")]);
    let git_floors = floors(&plan);
    assert!(git_floors.contains(&("/home/u/p/app/.git/config".into(), FloorKind::GitConfig)));
    assert!(git_floors.contains(&("/home/u/p/app/.git/hooks".into(), FloorKind::GitHooks)));
    assert!(!git_floors.iter().any(|(path, _)| path.starts_with("/home/u/p/app/sub")));
}

#[test]
fn plan_binds_gitfile_read_only() {
    let mut fs = world();
    fs.remove("/home/u/p/app/.git")
        .file("/home/u/p/app/.git", "gitdir: /home/u/p/main/.git/worktrees/app\n");
    let plan = plan(&spec(), &fs);
    assert!(has_mount(&plan, &ro("/home/u/p/app/.git")));
    assert!(!plan.mounts().iter().any(|mount| mount.origin == MountOrigin::Pin));
}

#[test]
fn plan_pins_registered_git_dirs_only_inside_write_roots() {
    let mut spec = spec();
    spec.git_dirs = vec!["/home/u/p/main/.git".into(), "/home/u/other/.git".into()];
    spec.write_roots
        .push(WriteRoot { path: "/home/u/p/main/.git".into(), kind: WriteRootKind::GitCommonDir });
    let mut fs = world();
    fs.dir("/home/u/p/main/.git/worktrees/app")
        .file("/home/u/p/main/.git/worktrees/app/commondir", "../..");
    fs.dir("/home/u/other/.git");
    fs.file("/home/u/p/main/.git/config", "").file("/home/u/other/.git/config", "");
    let plan = plan(&spec, &fs);
    // The common dir is a write root, which is its pin too: one mount point.
    assert!(has_mount(&plan, &rw("/home/u/p/main/.git")));
    assert!(has_mount(&plan, &ro("/home/u/p/main/.git/config")));
    // A git dir outside every write root is never bound writable.
    assert!(!plan.mounts().iter().any(|mount| mount.op.target().starts_with("/home/u/other")));
    assert!(plan.notes().contains(&PlanNote::MissingRoot("/home/u/other/.git".into())));
}

#[test]
fn plan_floors_git_includes_and_hooks_path_in_roots() {
    let mut fs = world();
    fs.file(
        "/home/u/p/app/.git/config",
        "[include]\n\tpath = ../shared.gitconfig\n[core]\n\thooksPath = .husky/_\n[includeIf \"gitdir:~/w/\"]\n\tpath = /etc/gitconfig.d/x\n",
    )
    .file("/home/u/p/app/shared.gitconfig", "")
    .dir("/home/u/p/app/.husky/_");
    let plan = plan(&spec(), &fs);
    let git_floors = floors(&plan);
    assert!(git_floors.contains(&("/home/u/p/app/shared.gitconfig".into(), FloorKind::GitConfig)));
    assert!(git_floors.contains(&("/home/u/p/app/.husky/_".into(), FloorKind::GitHooks)));
}

#[test]
fn plan_overlays_caches_with_pins_and_skips_them_when_read_only() {
    let spec = spec();
    let plan = plan(&spec, &world());
    let cache = &spec.caches[0];
    assert!(has_mount(
        &plan,
        &MountOp::Overlay {
            lower: "/home/u/.cargo".into(),
            upper: cache.upper.clone(),
            work: cache.work.clone(),
            target: "/home/u/.cargo".into(),
        }
    ));
    assert!(has_mount(&plan, &ro("/home/u/.cargo/bin")));
    assert!(has_mount(&plan, &ro("/home/u/.cargo/config.toml")));
    assert_eq!(
        plan.explain(Path::new("/home/u/.cargo/registry")).role,
        SandboxPathRole::CacheOverlay
    );

    let mut readonly = crate::testing::spec();
    readonly.cache_mode = CacheMode::Readonly;
    let plan = MountPlan::build(&readonly, &world()).unwrap();
    assert!(!plan.mounts().iter().any(|mount| mount.origin == MountOrigin::Cache));
    assert_eq!(plan.explain(Path::new("/home/u/.cargo/registry")).role, SandboxPathRole::ReadOnly);
}

#[test]
fn plan_unmasks_one_mask_but_never_an_engine_secret() {
    let mut spec = spec();
    spec.masks.push(Mask { path: "/home/u/p/app/.env".into(), kind: MaskKind::ProjectEnv });
    spec.grants.push(Grant::Unmask { path: "/home/u/p/app/.env".into() });
    let mut fs = world();
    fs.file("/home/u/p/app/.env", "KEY=1");
    let plan = plan(&spec, &fs);
    assert!(!has_mount(&plan, &MountOp::DevNull { target: "/home/u/p/app/.env".into() }));

    let mut spec = crate::testing::spec();
    spec.grants.push(Grant::Unmask { path: "/home/u/.ssh".into() });
    assert!(matches!(MountPlan::build(&spec, &world()), Err(SandboxError::UnmaskSecret { .. })));
}

#[test]
fn plan_grants_bind_sockets_devices_and_open_the_network() {
    let mut spec = spec();
    assert!(plan(&spec, &world()).unshares_network());
    spec.grants = vec![
        Grant::OpenNetwork,
        Grant::Bus { bus: BusKind::Session },
        Grant::Device { path: "/dev/nvme0n1".into() },
    ];
    let plan = plan(&spec, &world());
    assert!(!plan.unshares_network());
    assert!(has_mount(&plan, &rw("/run/user/1000/bus")));
    assert!(has_mount(&plan, &MountOp::DevBind { node: "/dev/nvme0n1".into() }));
    assert_eq!(
        plan.env_overrides(),
        [("DBUS_SESSION_BUS_ADDRESS".to_owned(), "unix:path=/run/user/1000/bus".to_owned())]
    );
    // The masks stay with an open network.
    assert!(has_mount(&plan, &MountOp::Tmpfs { target: "/home/u/.ssh".into(), perms: 0o500 }));

    let mut spec = crate::testing::spec();
    spec.grants = vec![Grant::Device { path: "/home/u/p/app/src".into() }];
    assert!(matches!(MountPlan::build(&spec, &world()), Err(SandboxError::GrantTarget { .. })));
}

#[test]
fn plan_binds_the_launcher_files_and_refuses_a_missing_one() {
    let spec = spec();
    let plan = plan(&spec, &world());
    let inside = spec.runtime.inside_dir();
    assert!(has_mount(
        &plan,
        &MountOp::Bind {
            source: spec.runtime.launcher.clone(),
            target: inside.join("efr-sbx"),
            writable: false
        }
    ));
    assert!(has_mount(
        &plan,
        &MountOp::Bind {
            source: spec.runtime.call_dir.join("line"),
            target: inside.join("line"),
            writable: false,
        }
    ));
    // state.zsh does not exist before the first call.
    assert!(!plan.mounts().iter().any(|mount| mount.op.target() == inside.join("state.zsh")));
    assert_eq!(plan.child_argv(), ["/usr/bin/zsh", "-f", "/run/user/1000/efr-sbx/child.zsh"]);

    let mut fs = world();
    fs.remove(&spec.runtime.call_dir.join("line").to_string_lossy());
    assert!(matches!(MountPlan::build(&spec, &fs), Err(SandboxError::MissingAsset { .. })));
}

#[test]
fn a_call_starts_in_scratch_when_the_shell_is_hidden() {
    let spec = spec();
    let mut fs = world();
    fs.dir("/tmp/work");
    let plan = plan(&spec, &fs);
    let scratch = &spec.runtime.scratch;
    let here = plan.start_dir(Path::new(PROJECT), None, scratch, &fs);
    assert_eq!(here, StartDir::Here { path: PROJECT.into() });
    let hidden = plan.start_dir(Path::new("/home/u/.ssh"), None, scratch, &fs);
    assert_eq!(hidden, StartDir::Scratch { path: scratch.clone(), hidden: "/home/u/.ssh".into() });
    let host_tmp = plan.start_dir(Path::new("/tmp/work"), None, scratch, &fs);
    assert!(matches!(host_tmp, StartDir::Scratch { .. }));
}

#[test]
fn a_call_starts_in_the_private_tmp_where_the_last_one_ended() {
    let spec = spec();
    let mut fs = world();
    fs.dir(&spec.runtime.private_tmp().join("work").to_string_lossy());
    let plan = plan(&spec, &fs);
    let last = SandboxCwd { path: "/tmp/work".into(), shell_pwd: PROJECT.into() };
    let scratch = &spec.runtime.scratch;
    let start = plan.start_dir(Path::new(PROJECT), Some(&last), scratch, &fs);
    assert_eq!(start, StartDir::Private { path: "/tmp/work".into() });
    assert_eq!(start.path(), Path::new("/tmp/work"));
    // The user moved the shell since: its directory wins.
    let moved = plan.start_dir(Path::new("/home/u/p/app/src"), Some(&last), scratch, &fs);
    assert_eq!(moved, StartDir::Here { path: "/home/u/p/app/src".into() });
}

#[test]
fn explain_names_the_role_of_each_kind_of_path() {
    let spec = spec();
    let plan = plan(&spec, &world());
    assert_eq!(
        plan.explain(Path::new("/home/u/p/app/src/main.rs")).role,
        SandboxPathRole::WriteRoot
    );
    assert_eq!(
        plan.explain(Path::new("/home/u/p/app/.git/hooks/pre-commit")).role,
        SandboxPathRole::Floor
    );
    assert_eq!(plan.explain(Path::new("/tmp/x")).role, SandboxPathRole::PrivateTmp);
    assert_eq!(plan.explain(Path::new("/etc/hosts")).role, SandboxPathRole::ReadOnly);
    assert_eq!(
        plan.explain(Path::new("/run/user/1000/efr/daemon.sock")).role,
        SandboxPathRole::Masked
    );
    let zshrc = plan.explain(Path::new("/home/u/.zshrc"));
    assert_eq!((zshrc.read, zshrc.write), (true, false));
}

#[test]
fn the_private_tmp_and_the_launcher_files_follow_links_of_efrs_own_roots() {
    let spec = spec();
    let mut fs = world();
    let real_tmp = "/data/state/efr/sandbox/tmp";
    fs.remove(&spec.runtime.private_tmp().to_string_lossy())
        .dir(real_tmp)
        .link(&spec.runtime.private_tmp().to_string_lossy(), real_tmp);
    let plan = plan(&spec, &fs);
    assert!(has_mount(
        &plan,
        &MountOp::Bind { source: real_tmp.into(), target: "/tmp".into(), writable: true }
    ));
}

#[test]
fn a_registered_git_dir_reached_through_a_link_in_a_root_is_refused() {
    let mut spec = spec();
    spec.git_dirs = vec!["/home/u/p/app/gd".into()];
    let mut fs = world();
    fs.dir("/home/u/p/app/real-git").link("/home/u/p/app/gd", "real-git");
    assert!(matches!(MountPlan::build(&spec, &fs), Err(SandboxError::SymlinkedPin { .. })));
}
