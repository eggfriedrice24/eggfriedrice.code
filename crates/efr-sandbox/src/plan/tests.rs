use std::path::{Path, PathBuf};

use efr_protocol::{BusKind, CacheMode, DesktopKind, Grant, SandboxPathRole};
use pretty_assertions::assert_eq;

use crate::paths::depth;
use crate::plan::{MountOp, MountOrigin, MountPlan, OpKind, PlanNote, StartDir};
use crate::spec::{DesktopSocket, Floor, FloorKind, Mask, MaskKind, WriteRoot, WriteRootKind};
use crate::state::SandboxCwd;
use crate::testing::{FakeFs, PROJECT, spec, world};
use crate::{InnerPolicy, NetworkPlan, SandboxError, SandboxSpec, SeccompProfile};

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

/// A module git dir as git makes it, at `.git/modules/<name>` of the project, with
/// `config` as its config.
fn module(fs: &mut FakeFs, name: &str, config: &str) {
    let dir = format!("/home/u/p/app/.git/modules/{name}");
    fs.file(&format!("{dir}/HEAD"), "ref: refs/heads/main\n")
        .file(&format!("{dir}/config"), config)
        .dir(&format!("{dir}/hooks"))
        .dir(&format!("{dir}/objects/pack"));
}

fn has_git_floors(plan: &MountPlan, git_dir: &str) -> bool {
    let git_floors = floors(plan);
    git_floors.contains(&(format!("{git_dir}/config").into(), FloorKind::GitConfig))
        && git_floors.contains(&(format!("{git_dir}/hooks").into(), FloorKind::GitHooks))
}

#[test]
fn pin_covers_slash_named_submodule() {
    // `git submodule add <url> vendor/foo` keeps its git dir at
    // `.git/modules/vendor/foo`; one level below `modules/` is only `vendor`.
    let mut fs = world();
    module(&mut fs, "vendor/foo", "[core]\n\tworktree = ../../../../vendor/foo\n");
    module(&mut fs, "lib", "");
    let plan = plan(&spec(), &fs);
    assert!(has_git_floors(&plan, "/home/u/p/app/.git/modules/vendor/foo"), "{plan:#?}");
    assert!(has_git_floors(&plan, "/home/u/p/app/.git/modules/lib"), "{plan:#?}");
    assert!(
        !floors(&plan)
            .iter()
            .any(|(path, _)| path.starts_with("/home/u/p/app/.git/modules/vendor/foo/objects"))
    );

    // A link at any depth of `modules/` refuses the plan, as one at the first did.
    let mut fs = world();
    module(&mut fs, "a", "");
    fs.dir("/home/u/p/evil/hooks").link("/home/u/p/app/.git/modules/vendor/foo", "/home/u/p/evil");
    let error = MountPlan::build(&spec(), &fs).unwrap_err();
    assert!(matches!(error, SandboxError::SymlinkedPin { .. }), "{error:?}");
}

#[test]
fn pin_covers_nested_module() {
    let mut fs = world();
    module(&mut fs, "a", "");
    module(&mut fs, "a/modules/b", "");
    module(&mut fs, "a/modules/b/modules/c", "");
    let plan = plan(&spec(), &fs);
    for git_dir in ["a", "a/modules/b", "a/modules/b/modules/c"] {
        let git_dir = format!("/home/u/p/app/.git/modules/{git_dir}");
        assert!(has_git_floors(&plan, &git_dir), "{git_dir}: {plan:#?}");
    }
}

#[test]
fn module_hooks_path_resolves_against_its_worktree() {
    // git writes `core.worktree` into each module's config: a relative hooks path is
    // relative to that work tree, not to the git dir's parent.
    let mut fs = world();
    module(
        &mut fs,
        "vendor/foo",
        "[core]\n\tworktree = ../../../../vendor/foo\n\thooksPath = .githooks\n[include]\n\tpath = ../../../../shared.gitconfig\n",
    );
    fs.dir("/home/u/p/app/vendor/foo/.githooks")
        .dir("/home/u/p/app/.git/modules/vendor/.githooks")
        .file("/home/u/p/app/shared.gitconfig", "");
    let built = plan(&spec(), &fs);
    let git_floors = floors(&built);
    assert!(
        git_floors.contains(&("/home/u/p/app/vendor/foo/.githooks".into(), FloorKind::GitHooks)),
        "{git_floors:#?}"
    );
    assert!(!git_floors.iter().any(|(path, _)| path.ends_with("modules/vendor/.githooks")));
    assert!(git_floors.contains(&("/home/u/p/app/shared.gitconfig".into(), FloorKind::GitConfig)));

    // With no `core.worktree`, the path that the superproject's `.gitmodules` gives
    // the module's name is its work tree; for a nested module, the `.gitmodules` of
    // the module above it.
    let mut fs = world();
    module(&mut fs, "lib", "[core]\n\thooksPath = hooks.d\n");
    module(&mut fs, "lib/modules/inner", "[core]\n\thooksPath = git/hooks\n");
    fs.file("/home/u/p/app/.gitmodules", "[submodule \"lib\"]\n\tpath = third_party/lib\n")
        .file(
            "/home/u/p/app/third_party/lib/.gitmodules",
            "[submodule \"inner\"]\n\tpath = deps/inner\n",
        )
        .dir("/home/u/p/app/third_party/lib/hooks.d")
        .dir("/home/u/p/app/third_party/lib/deps/inner/git/hooks");
    let plan = plan(&spec(), &fs);
    let git_floors = floors(&plan);
    for hooks in ["third_party/lib/hooks.d", "third_party/lib/deps/inner/git/hooks"] {
        let hooks = PathBuf::from(format!("/home/u/p/app/{hooks}"));
        assert!(git_floors.contains(&(hooks.clone(), FloorKind::GitHooks)), "{hooks:?}");
    }
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
fn plan_gives_the_helper_each_overlay_with_the_mounts_inside_it() {
    let mut spec = spec();
    spec.masks
        .push(Mask { path: "/home/u/.cargo/credentials.toml".into(), kind: MaskKind::SandboxMask });
    let mut fs = world();
    fs.file("/home/u/.cargo/credentials.toml", "token");
    let plan = plan(&spec, &fs);
    let layers = plan.cache_layers().unwrap();
    assert_eq!(layers.staging, Path::new("/run/user/1000/efr-sbx/layers"));
    let [layer] = layers.layers.as_slice() else { panic!("{layers:#?}") };
    assert_eq!(layer.target, Path::new("/home/u/.cargo"));
    assert!(!layer.fresh);
    assert_eq!(layer.dir, Path::new("/run/user/1000/efr-sbx/layers/0"));
    assert_eq!(layer.upper, layer.dir.join("upper"));
    assert_eq!(layer.work, layer.dir.join("work"));
    // The mask and the pins inside the cache move onto the overlay, so they still win.
    let mut moved = layer.moved.clone();
    moved.sort();
    assert_eq!(
        moved,
        [
            PathBuf::from("/home/u/.cargo/bin"),
            PathBuf::from("/home/u/.cargo/config.toml"),
            PathBuf::from("/home/u/.cargo/credentials.toml"),
        ]
    );
    assert_eq!(plan.layer_sources, [spec.caches[0].upper.parent().unwrap().to_path_buf()]);

    let mut tmp = crate::testing::spec();
    tmp.cache_mode = CacheMode::Tmp;
    let plan = MountPlan::build(&tmp, &world()).unwrap();
    let layers = plan.cache_layers().unwrap();
    assert!(layers.layers.iter().all(|layer| layer.fresh));
    assert!(plan.layer_sources.is_empty());

    let mut readonly = crate::testing::spec();
    readonly.cache_mode = CacheMode::Readonly;
    assert_eq!(MountPlan::build(&readonly, &world()).unwrap().cache_layers(), None);
}

#[test]
fn plan_refuses_two_caches_on_one_layer_dir() {
    let mut spec = spec();
    let shared = crate::CacheOverlay { target: "/home/u/.npm".into(), ..spec.caches[0].clone() };
    spec.caches.push(shared);
    let mut fs = world();
    fs.dir("/home/u/.npm");
    let error = MountPlan::build(&spec, &fs).unwrap_err();
    assert!(matches!(error, SandboxError::CacheLayer { .. }), "{error:?}");

    let mut spec = crate::testing::spec();
    spec.caches[0].work = "/home/u/.local/state/efr/elsewhere/work".into();
    let error = MountPlan::build(&spec, &world()).unwrap_err();
    assert!(matches!(error, SandboxError::CacheLayer { .. }), "{error:?}");

    // In the tmp mode the layer dirs are not used.
    let mut spec = crate::testing::spec();
    spec.cache_mode = CacheMode::Tmp;
    spec.caches[0].work = "/home/u/.local/state/efr/elsewhere/work".into();
    assert!(MountPlan::build(&spec, &world()).is_ok());
}

/// The spec of [`spec`] in the `tmp` mode with `~/.npm` first and the kept `~/.cargo`
/// second (`sandbox.keep_downloads`), and its machine.
fn tmp_with_kept_cargo() -> (SandboxSpec, FakeFs) {
    let mut spec = spec();
    spec.cache_mode = CacheMode::Tmp;
    spec.caches[0].keep = true;
    let runtime = &spec.runtime;
    let npm =
        crate::CacheOverlay::new(Path::new("/home/u/.npm"), &runtime.sandbox_dir, &runtime.home);
    spec.caches.insert(0, npm);
    let mut fs = world();
    fs.dir("/home/u/.npm");
    (spec, fs)
}

#[test]
fn keep_cache_is_an_overlay_in_tmp_mode() {
    let (spec, fs) = tmp_with_kept_cargo();
    let plan = plan(&spec, &fs);
    let cargo = &spec.caches[1];
    assert!(has_mount(
        &plan,
        &MountOp::Overlay {
            lower: "/home/u/.cargo".into(),
            upper: cargo.upper.clone(),
            work: cargo.work.clone(),
            target: "/home/u/.cargo".into(),
        }
    ));
    // The pins hold as in every mode.
    assert!(has_mount(&plan, &ro("/home/u/.cargo/bin")));
    assert!(has_mount(&plan, &ro("/home/u/.cargo/config.toml")));
    assert_eq!(
        plan.explain(Path::new("/home/u/.cargo/registry")).role,
        SandboxPathRole::CacheOverlay
    );
    // The layer dir check of the `overlay` mode holds for a kept cache too.
    let mut shared = spec.clone();
    shared.caches[0] = crate::CacheOverlay {
        target: "/home/u/.npm".into(),
        keep: true,
        ..shared.caches[1].clone()
    };
    let error = MountPlan::build(&shared, &fs).unwrap_err();
    assert!(matches!(error, SandboxError::CacheLayer { .. }), "{error:?}");
    // `keep` means nothing in the `readonly` mode.
    let mut readonly = spec;
    readonly.cache_mode = CacheMode::Readonly;
    let plan = MountPlan::build(&readonly, &fs).unwrap();
    assert!(!plan.mounts().iter().any(|mount| mount.origin == MountOrigin::Cache));
}

#[test]
fn keep_cache_layer_is_not_fresh() {
    let (spec, fs) = tmp_with_kept_cargo();
    let plan = plan(&spec, &fs);
    let layers = plan.cache_layers().unwrap();
    let cargo = layers.layers.iter().find(|layer| layer.target == Path::new("/home/u/.cargo"));
    let cargo = cargo.unwrap();
    assert!(!cargo.fresh, "{layers:#?}");
    assert_eq!(cargo.upper, cargo.dir.join("upper"));
    assert_eq!(cargo.work, cargo.dir.join("work"));
    // The launcher locks the parent of each layer dir, so the conversation's layer
    // lock also covers the kept cache in the `tmp` mode.
    let layer = spec.caches[1].upper.parent().unwrap().to_path_buf();
    assert_eq!(plan.layer_dirs(), [layer]);
    assert_eq!(
        plan.layer_dirs()[0].parent(),
        Some(spec.runtime.sandbox_dir.join("cache").as_path())
    );
}

#[test]
fn other_caches_stay_tmp() {
    let (spec, fs) = tmp_with_kept_cargo();
    let plan = plan(&spec, &fs);
    assert!(has_mount(
        &plan,
        &MountOp::TmpOverlay { lower: "/home/u/.npm".into(), target: "/home/u/.npm".into() }
    ));
    let layers = plan.cache_layers().unwrap();
    let fresh: Vec<(&Path, bool)> =
        layers.layers.iter().map(|layer| (layer.target.as_path(), layer.fresh)).collect();
    assert_eq!(fresh, [(Path::new("/home/u/.npm"), true), (Path::new("/home/u/.cargo"), false)]);
    // bwrap binds the one kept layer dir onto the kept cache's staging dir, never onto
    // the fresh one's.
    let mut fds = crate::FdTable::new(&fs);
    let args: Vec<String> = plan
        .bwrap_args(&mut fds, &crate::LaunchFds { status: 9, policy: 5 }, Path::new(PROJECT))
        .unwrap()
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let chdir = args.iter().position(|arg| arg == "--chdir").unwrap();
    let staging = "/run/user/1000/efr-sbx/layers";
    assert_eq!(args[chdir - 7..chdir - 3], ["--perms", "0700", "--tmpfs", staging]);
    assert_eq!(args[chdir - 3], "--bind-fd");
    assert_eq!(args[chdir - 1], format!("{staging}/1"));

    // With no kept cache, the `tmp` mode is today's: every layer fresh, no layer dir.
    let mut today = spec.clone();
    today.caches[1].keep = false;
    let plan = MountPlan::build(&today, &fs).unwrap();
    assert!(plan.cache_layers().unwrap().layers.iter().all(|layer| layer.fresh));
    assert!(plan.layer_dirs().is_empty());
    assert!(!plan.mounts().iter().any(|mount| matches!(mount.op, MountOp::Overlay { .. })));
    // In the `overlay` mode every cache keeps its layer, kept or not.
    let mut overlay = today;
    overlay.cache_mode = CacheMode::Overlay;
    let mut fs = fs;
    let npm = &overlay.caches[0];
    fs.dir(&npm.upper.to_string_lossy()).dir(&npm.work.to_string_lossy());
    let plan = MountPlan::build(&overlay, &fs).unwrap();
    assert!(plan.cache_layers().unwrap().layers.iter().all(|layer| !layer.fresh));
    assert_eq!(plan.layer_dirs().len(), 2);
}

#[test]
fn plan_drops_the_overlay_of_a_cache_that_a_floor_covers() {
    let mut spec = spec();
    spec.floors.push(Floor { path: "/home/u/.cargo".into(), kind: FloorKind::User });
    let plan = plan(&spec, &world());
    assert!(!plan.mounts().iter().any(|mount| mount.origin == MountOrigin::Cache));
    assert!(has_mount(&plan, &ro("/home/u/.cargo")));
    assert!(plan.notes().contains(&PlanNote::CacheSkipped("/home/u/.cargo".into())));
    assert_eq!(plan.cache_layers(), None);
    assert!(!plan.write_dirs().iter().any(|dir| dir == Path::new("/home/u/.cargo")));
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
    // explain says what the binds do: the call can write a granted socket or device.
    for path in ["/run/user/1000/bus", "/dev/nvme0n1"] {
        let explained = plan.explain(Path::new(path));
        assert_eq!(
            (explained.role, explained.read, explained.write),
            (SandboxPathRole::Granted, true, true),
            "{path}"
        );
    }

    let mut spec = crate::testing::spec();
    spec.grants = vec![Grant::Device { path: "/home/u/p/app/src".into() }];
    assert!(matches!(MountPlan::build(&spec, &world()), Err(SandboxError::GrantTarget { .. })));
}

#[test]
fn plan_refuses_a_socket_below_a_secret_or_in_efrs_roots() {
    let mut fs = world();
    let base = spec();
    let efrd = base.runtime.runtime.join("daemon.sock");
    fs.socket("/home/u/.ssh/agent.sock").socket(&efrd.display().to_string());
    fs.socket("/run/user/1000/app.sock");
    for refused in [PathBuf::from("/home/u/.ssh/agent.sock"), efrd] {
        let mut spec = base.clone();
        spec.grants = vec![Grant::Socket { path: refused.clone() }];
        assert!(
            matches!(MountPlan::build(&spec, &fs), Err(SandboxError::GrantTarget { .. })),
            "{refused:?}"
        );
    }
    // A socket in the user's runtime dir, which the plan masks as a whole, still binds.
    let mut spec = base;
    spec.grants = vec![Grant::Socket { path: "/run/user/1000/app.sock".into() }];
    assert!(has_mount(&plan(&spec, &fs), &rw("/run/user/1000/app.sock")));
}

const SIGNATURE: &str = "0b4a3e5a_1760000000_1234";
const HYPR_SOCKET: &str = "/run/user/1000/hypr/0b4a3e5a_1760000000_1234/.socket.sock";

fn hyprland() -> DesktopSocket {
    DesktopSocket {
        desktop: DesktopKind::Hyprland,
        path: HYPR_SOCKET.into(),
        value: SIGNATURE.to_owned(),
    }
}

#[test]
fn plan_desktop_grant_binds_socket_and_sets_env() {
    let mut fs = world();
    fs.socket(HYPR_SOCKET)
        .socket("/run/user/1000/hypr/0b4a3e5a_1760000000_1234/.socket2.sock")
        .file("/run/user/1000/hypr/0b4a3e5a_1760000000_1234/hyprland.log", "titles")
        .socket("/tmp/.X11-unix/X0");
    let mut spec = spec();
    let x11 = DesktopSocket {
        desktop: DesktopKind::X11,
        path: "/tmp/.X11-unix/X0".into(),
        value: ":0".into(),
    };
    spec.runtime.desktops = vec![hyprland(), x11];
    // Without a grant, a socket of the spec binds nothing.
    let plain = plan(&spec, &fs);
    assert!(!plain.mounts().iter().any(|mount| mount.origin == MountOrigin::Socket));
    assert!(plain.env_overrides().is_empty());

    spec.grants = vec![Grant::Desktop { desktop: DesktopKind::Hyprland }];
    let plan = plan(&spec, &fs);
    let sockets: Vec<&MountOp> = plan
        .mounts()
        .iter()
        .filter(|mount| mount.origin == MountOrigin::Socket)
        .map(|mount| &mount.op)
        .collect();
    assert_eq!(sockets, [&rw(HYPR_SOCKET)], "only the control socket, never the dir");
    assert_eq!(plan.resolve_unix, [PathBuf::from(HYPR_SOCKET)]);
    assert_eq!(
        plan.env_overrides(),
        [("HYPRLAND_INSTANCE_SIGNATURE".to_owned(), SIGNATURE.to_owned())]
    );
    // The bind lands inside the masked runtime dir, after its tmpfs.
    let at = |target: &str| {
        plan.mounts().iter().position(|mount| mount.op.target() == Path::new(target)).unwrap()
    };
    assert!(at("/run/user/1000") < at(HYPR_SOCKET));

    spec.grants = vec![Grant::Desktop { desktop: DesktopKind::X11 }];
    let plan = MountPlan::build(&spec, &fs).unwrap();
    assert!(has_mount(&plan, &rw("/tmp/.X11-unix/X0")));
    assert_eq!(plan.env_overrides(), [("DISPLAY".to_owned(), ":0".to_owned())]);
}

#[test]
fn plan_refuses_desktop_socket_outside_runtime_or_x11_dir() {
    let mut fs = world();
    fs.socket(HYPR_SOCKET)
        .socket("/run/user/1000/hypr/0b4a3e5a_1760000000_1234/.socket2.sock")
        .socket("/home/u/p/app/fake.sock")
        .dir("/home/u/p/app/hypr")
        .link("/run/user/1000/hypr/linked", "/home/u/p/app/hypr")
        .socket("/home/u/p/app/hypr/.socket.sock")
        .file("/run/user/1000/wayland-9", "")
        .socket("/run/user/1000/bus")
        .link("/run/user/1000/wayland-3", "/run/user/1000/bus")
        .socket("/run/user/1000/sway-ipc.1000.42.sock");
    let refused = [
        // The event socket, though the spec names it.
        DesktopSocket {
            desktop: DesktopKind::Hyprland,
            path: "/run/user/1000/hypr/0b4a3e5a_1760000000_1234/.socket2.sock".into(),
            value: SIGNATURE.to_owned(),
        },
        // A path that its value does not name.
        DesktopSocket {
            desktop: DesktopKind::Wayland,
            path: "/home/u/p/app/fake.sock".into(),
            value: "wayland-0".to_owned(),
        },
        // A value that names a path outside the runtime dir.
        DesktopSocket {
            desktop: DesktopKind::Sway,
            path: "/home/u/p/app/fake.sock".into(),
            value: "/home/u/p/app/fake.sock".to_owned(),
        },
        // A link in the runtime dir to a socket in a write root.
        DesktopSocket {
            desktop: DesktopKind::Hyprland,
            path: "/run/user/1000/hypr/linked/.socket.sock".into(),
            value: "linked".to_owned(),
        },
        // Not a socket.
        DesktopSocket {
            desktop: DesktopKind::Wayland,
            path: "/run/user/1000/wayland-9".into(),
            value: "wayland-9".to_owned(),
        },
        // A display on another host.
        DesktopSocket {
            desktop: DesktopKind::X11,
            path: "/tmp/.X11-unix/X0".into(),
            value: "host:0".to_owned(),
        },
        // Another socket of the user, though the spec names it.
        DesktopSocket {
            desktop: DesktopKind::Sway,
            path: "/run/user/1000/bus".into(),
            value: "/run/user/1000/bus".to_owned(),
        },
        // A link with a desktop's name to another socket of the user.
        DesktopSocket {
            desktop: DesktopKind::Wayland,
            path: "/run/user/1000/wayland-3".into(),
            value: "wayland-3".to_owned(),
        },
    ];
    for socket in refused {
        let mut spec = spec();
        spec.grants = vec![Grant::Desktop { desktop: socket.desktop }];
        spec.runtime.desktops = vec![socket.clone()];
        assert!(
            matches!(MountPlan::build(&spec, &fs), Err(SandboxError::GrantTarget { .. })),
            "{socket:?}"
        );
    }
    let mut spec = spec();
    spec.grants = vec![Grant::Desktop { desktop: DesktopKind::Sway }];
    spec.runtime.desktops = vec![DesktopSocket {
        desktop: DesktopKind::Sway,
        path: "/run/user/1000/sway-ipc.1000.42.sock".into(),
        value: "/run/user/1000/sway-ipc.1000.42.sock".to_owned(),
    }];
    assert!(has_mount(&plan(&spec, &fs), &rw("/run/user/1000/sway-ipc.1000.42.sock")));
}

/// The plan of the spec with the one grant `Grant::Socket { path }`.
fn socket_grant(fs: &FakeFs, path: &str) -> Result<MountPlan, SandboxError> {
    let mut spec = spec();
    spec.grants = vec![Grant::Socket { path: path.into() }];
    MountPlan::build(&spec, fs)
}

/// Fails unless `result` refuses the socket grant of `path`.
fn refused(result: &Result<MountPlan, SandboxError>, path: &str) {
    assert!(
        matches!(result, Err(SandboxError::SocketGrantRefused { path: named }) if named == Path::new(path)),
        "{path}: {result:?}"
    );
}

#[test]
fn socket_grant_link_to_bus_refused() {
    let mut fs = world();
    fs.link("/home/u/p/app/app.sock", "/run/user/1000/bus")
        .link("/home/u/p/app/sys.sock", "/run/dbus/system_bus_socket")
        .link("/var/run", "/run")
        .link("/home/u/p/app/var.sock", "/var/run/dbus/system_bus_socket")
        .socket("/run/user/1000/app.sock");
    for path in [
        "/home/u/p/app/app.sock",
        "/home/u/p/app/sys.sock",
        "/home/u/p/app/var.sock",
        "/run/user/1000/bus",
        "/run/dbus/system_bus_socket",
    ] {
        refused(&socket_grant(&fs, path), path);
    }
    // The session bus at $XRT/bus is refused also when the spec names no session bus,
    // and the system bus that the spec names is refused wherever it lies.
    let mut spec = spec();
    spec.runtime.session_bus = None;
    spec.grants = vec![Grant::Socket { path: "/home/u/p/app/app.sock".into() }];
    refused(&MountPlan::build(&spec, &fs), "/home/u/p/app/app.sock");
    fs.socket("/home/u/test-bus/bus").link("/home/u/p/app/test.sock", "/home/u/test-bus/bus");
    let mut spec = crate::testing::spec();
    spec.runtime.system_bus = "/home/u/test-bus/bus".into();
    spec.grants = vec![Grant::Socket { path: "/home/u/p/app/test.sock".into() }];
    refused(&MountPlan::build(&spec, &fs), "/home/u/p/app/test.sock");
    // Another socket of the runtime dir still binds.
    let bound = socket_grant(&fs, "/run/user/1000/app.sock").unwrap();
    assert!(has_mount(&bound, &rw("/run/user/1000/app.sock")));
    // A bus grant still binds its bus.
    let mut spec = crate::testing::spec();
    spec.grants = vec![Grant::Bus { bus: BusKind::System }];
    assert!(has_mount(&plan(&spec, &fs), &rw("/run/dbus/system_bus_socket")));
}

#[test]
fn socket_grant_to_service_socket_refused() {
    let mut fs = world();
    fs.socket("/run/user/1000/systemd/private")
        .socket("/run/user/1000/systemd/notify")
        .socket("/run/systemd/private")
        .socket("/run/systemd/io.systemd.Manager")
        .socket("/run/docker.sock")
        .socket("/run/user/1000/podman/podman.sock")
        .socket("/run/libvirt/virtqemud-sock")
        .socket("/home/u/p/app/run.sock")
        .link("/home/u/p/app/m.sock", "/run/user/1000/systemd/notify")
        .link("/home/u/p/app/d.sock", "/run/docker.sock")
        .link("/home/u/p/app/docker.sock", "/home/u/p/app/run.sock");
    for path in [
        "/run/user/1000/systemd/private",
        "/run/user/1000/systemd/notify",
        "/run/systemd/private",
        "/run/systemd/io.systemd.Manager",
        "/run/docker.sock",
        "/run/user/1000/podman/podman.sock",
        "/run/libvirt/virtqemud-sock",
        "/home/u/p/app/m.sock",
        "/home/u/p/app/d.sock",
        // A link with the name of a service socket: the name alone refuses it.
        "/home/u/p/app/docker.sock",
    ] {
        refused(&socket_grant(&fs, path), path);
    }
    let bound = socket_grant(&fs, "/home/u/p/app/run.sock").unwrap();
    assert!(has_mount(&bound, &rw("/home/u/p/app/run.sock")));
}

#[test]
fn socket_grant_to_desktop_socket_refused() {
    let mut fs = world();
    fs.socket(HYPR_SOCKET)
        .socket("/run/user/1000/wayland-1")
        .socket("/run/user/1000/sway-ipc.1000.42.sock")
        .socket("/run/user/1000/niri.wayland-1.42.sock")
        .socket("/tmp/.X11-unix/X0")
        .link("/home/u/p/app/w.sock", "/run/user/1000/wayland-1")
        .link("/home/u/p/app/x.sock", "/tmp/.X11-unix/X0")
        .link("/home/u/p/app/h.sock", HYPR_SOCKET);
    for path in [
        HYPR_SOCKET,
        "/run/user/1000/wayland-1",
        "/run/user/1000/sway-ipc.1000.42.sock",
        "/run/user/1000/niri.wayland-1.42.sock",
        "/tmp/.X11-unix/X0",
        "/home/u/p/app/w.sock",
        "/home/u/p/app/x.sock",
        "/home/u/p/app/h.sock",
    ] {
        refused(&socket_grant(&fs, path), path);
    }
}

#[test]
fn desktop_grant_still_binds_its_socket() {
    let mut fs = world();
    fs.socket(HYPR_SOCKET).socket("/run/user/1000/wayland-1");
    let mut spec = spec();
    spec.runtime.desktops = vec![
        hyprland(),
        DesktopSocket {
            desktop: DesktopKind::Wayland,
            path: "/run/user/1000/wayland-1".into(),
            value: "wayland-1".to_owned(),
        },
    ];
    spec.grants = vec![Grant::Desktop { desktop: DesktopKind::Hyprland }];
    let hypr = plan(&spec, &fs);
    assert!(has_mount(&hypr, &rw(HYPR_SOCKET)));
    assert_eq!(hypr.resolve_unix, [PathBuf::from(HYPR_SOCKET)]);
    spec.grants = vec![Grant::Desktop { desktop: DesktopKind::Wayland }];
    let wayland = plan(&spec, &fs);
    assert!(has_mount(&wayland, &rw("/run/user/1000/wayland-1")));
    assert_eq!(wayland.env_overrides(), [("WAYLAND_DISPLAY".to_owned(), "wayland-1".to_owned())]);
}

#[test]
fn plan_desktop_without_socket_is_no_desktop() {
    let mut fs = world();
    fs.socket(HYPR_SOCKET);
    let mut spec = spec();
    spec.grants = vec![Grant::Desktop { desktop: DesktopKind::Wayland }];
    spec.runtime.desktops = vec![hyprland()];
    let error = MountPlan::build(&spec, &fs).unwrap_err();
    assert!(
        matches!(
            error,
            SandboxError::NoDesktop { desktop: DesktopKind::Wayland, needs_cookie: false }
        ),
        "{error:?}"
    );
    assert_eq!(error.to_string(), "efr found no Wayland session in the terminal of this prompt");
    let cookie = SandboxError::NoDesktop { desktop: DesktopKind::X11, needs_cookie: true };
    assert!(cookie.to_string().contains("needs an X cookie"), "{cookie}");
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
fn plan_binds_the_mount_snapshot_only_when_efrd_wrote_it() {
    let spec = spec();
    let inside = spec.runtime.inside_dir();
    let snapshot = |plan: &MountPlan| -> Vec<MountOp> {
        plan.mounts()
            .iter()
            .filter(|mount| mount.op.target().starts_with(&inside))
            .filter(|mount| {
                let name = mount.op.target().file_name().unwrap_or_default();
                name == "mounts" || name == "mounts.json"
            })
            .map(|mount| mount.op.clone())
            .collect()
    };
    // A line without df or findmnt gets no snapshot and no bind.
    assert_eq!(snapshot(&plan(&spec, &world())), []);

    let mut fs = world();
    for name in ["mounts", "mounts.json"] {
        fs.file(&spec.runtime.call_dir.join(name).to_string_lossy(), "");
    }
    let bound = snapshot(&plan(&spec, &fs));
    assert_eq!(
        bound,
        ["mounts", "mounts.json"].map(|name| MountOp::Bind {
            source: spec.runtime.call_dir.join(name),
            target: inside.join(name),
            writable: false,
        })
    );
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

/// A file system that counts how often each path is looked up.
struct Lookups<'a> {
    fs: &'a FakeFs,
    lstats: std::cell::RefCell<std::collections::BTreeMap<PathBuf, usize>>,
}

impl crate::FsView for Lookups<'_> {
    fn lstat(&self, path: &Path) -> std::io::Result<crate::FileKind> {
        *self.lstats.borrow_mut().entry(path.to_path_buf()).or_default() += 1;
        self.fs.lstat(path)
    }
    fn read_link(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.fs.read_link(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<std::ffi::OsString>> {
        self.fs.read_dir(path)
    }
    fn read_file(&self, path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
        self.fs.read_file(path, limit)
    }
    fn open_no_symlinks(&self, path: &Path) -> std::io::Result<std::os::fd::OwnedFd> {
        self.fs.open_no_symlinks(path)
    }
    fn open_empty(&self) -> std::io::Result<std::os::fd::OwnedFd> {
        self.fs.open_empty()
    }
}

#[test]
fn plan_looks_up_each_path_once() {
    let mut spec = spec();
    for n in 0..40 {
        let path = PathBuf::from(format!("/home/u/.config/app{n}/secret"));
        spec.masks.push(Mask { path, kind: MaskKind::SandboxMask });
        let path = PathBuf::from(format!("/home/u/p/app/.tool{n}"));
        spec.floors.push(Floor { path, kind: FloorKind::ProtectedName });
    }
    let mut fs = world();
    fs.dir("/home/u/.config/app7").file("/home/u/.config/app7/secret", "token");
    fs.link("/home/u/.config/app3", "/home/u/dotfiles/app3");
    let lookups = Lookups { fs: &fs, lstats: std::cell::RefCell::default() };
    let plan = MountPlan::build(&spec, &lookups).unwrap();
    assert!(has_mount(&plan, &MountOp::DevNull { target: "/home/u/.config/app7/secret".into() }));
    let lstats = lookups.lstats.borrow();
    let twice: Vec<(&PathBuf, &usize)> = lstats.iter().filter(|(_, n)| **n > 1).collect();
    assert!(twice.is_empty(), "looked up more than once: {twice:?}");
    assert_eq!(lstats.get(Path::new("/home/u")), Some(&1));
}

#[test]
fn plan_skips_a_cache_in_a_private_dir_of_the_sandbox() {
    let mut spec = spec();
    let runtime = spec.runtime.clone();
    for dir in ["/tmp/cache", "/var/tmp/cache"] {
        spec.caches.push(crate::CacheOverlay::new(
            Path::new(dir),
            &runtime.sandbox_dir,
            &runtime.home,
        ));
    }
    spec.cache_mode = CacheMode::Tmp;
    let mut fs = world();
    fs.dir("/tmp/cache").dir("/var/tmp/cache");
    let plan = plan(&spec, &fs);
    let layers = plan.cache_layers().unwrap();
    let targets: Vec<&Path> = layers.layers.iter().map(|layer| layer.target.as_path()).collect();
    assert_eq!(targets, [Path::new("/home/u/.cargo")]);
    assert!(plan.notes().contains(&PlanNote::CacheSkipped("/tmp/cache".into())));
    assert!(plan.notes().contains(&PlanNote::CacheSkipped("/var/tmp/cache".into())));
}

/// The cache targets of the helper's plan.
fn overlay_targets(plan: &MountPlan) -> Vec<PathBuf> {
    plan.cache_layers()
        .map(|layers| layers.layers.iter().map(|layer| layer.target.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn plan_overlays_only_the_outer_cache_when_the_inner_one_comes_first() {
    let mut spec = spec();
    let runtime = spec.runtime.clone();
    let cache = |target: &str| {
        crate::CacheOverlay::new(Path::new(target), &runtime.sandbox_dir, &runtime.home)
    };
    spec.caches = vec![cache("/home/u/.cache/pip"), cache("/home/u/.cache")];
    let mut fs = world();
    fs.dir("/home/u/.cache/pip");
    for overlay in &spec.caches {
        fs.dir(&overlay.upper.to_string_lossy()).dir(&overlay.work.to_string_lossy());
    }
    let plan = plan(&spec, &fs);
    assert_eq!(overlay_targets(&plan), [PathBuf::from("/home/u/.cache")]);
    assert!(plan.notes().contains(&PlanNote::CacheSkipped("/home/u/.cache/pip".into())));
    let layers = plan.cache_layers().unwrap();
    assert!(layers.layers[0].moved.is_empty(), "{layers:#?}");
}

#[test]
fn plan_keeps_the_pins_of_a_cache_that_links_into_another_cache() {
    let mut spec = spec();
    let runtime = spec.runtime.clone();
    let cache = |target: &str| {
        crate::CacheOverlay::new(Path::new(target), &runtime.sandbox_dir, &runtime.home)
    };
    // ~/.cargo is a link into ~/.cache, and comes first in the list.
    spec.caches = vec![cache("/home/u/.cargo"), cache("/home/u/.cache")];
    let mut fs = world();
    fs.link("/home/u/.cargo", "/home/u/.cache/cargo")
        .dir("/home/u/.cache/cargo/bin")
        .file("/home/u/.cache/cargo/config.toml", "");
    for overlay in &spec.caches {
        fs.dir(&overlay.upper.to_string_lossy()).dir(&overlay.work.to_string_lossy());
    }
    let plan = plan(&spec, &fs);
    assert_eq!(overlay_targets(&plan), [PathBuf::from("/home/u/.cache")]);
    assert!(plan.notes().contains(&PlanNote::CacheSkipped("/home/u/.cargo".into())));
    // The pins of ~/.cargo still win over the overlay of ~/.cache, and move onto it.
    assert!(has_mount(&plan, &ro("/home/u/.cache/cargo/bin")));
    assert!(has_mount(&plan, &ro("/home/u/.cache/cargo/config.toml")));
    let layers = plan.cache_layers().unwrap();
    let mut moved = layers.layers[0].moved.clone();
    moved.sort();
    assert_eq!(
        moved,
        [
            PathBuf::from("/home/u/.cache/cargo/bin"),
            PathBuf::from("/home/u/.cache/cargo/config.toml"),
        ]
    );
}

/// [`spec`] as a job: its call dir below the shell dir's `jobs`, with the line there.
fn job_spec(fs: &mut FakeFs) -> SandboxSpec {
    let mut spec = spec();
    spec.runtime.call_dir = spec.runtime.shell_dir.join(crate::JOB_DIR).join(crate::testing::CALL);
    spec.job = Some(crate::JobPlan::new(3600, 19));
    fs.file(&spec.runtime.call_dir.join("line").to_string_lossy(), "cargo watch");
    spec
}

/// The mounts of `plan` without the two job floors.
fn without_job_floors(plan: &MountPlan) -> Vec<(PathBuf, MountOrigin, OpKind)> {
    plan.mounts()
        .iter()
        .filter(|mount| {
            !matches!(mount.origin, MountOrigin::Floor(FloorKind::JobGitDir | FloorKind::JobOutput))
        })
        .map(|mount| (mount.op.target().to_path_buf(), mount.origin, mount.kind))
        .collect()
}

#[test]
fn plan_floors_the_job_output_dir_once_efrd_made_it() {
    let mut fs = world();
    let outputs = spec().runtime.job_outputs();
    let output_floor = (outputs.clone(), FloorKind::JobOutput);
    assert!(!floors(&plan(&spec(), &fs)).contains(&output_floor));
    // A link that a call left there is not efrd's dir: no floor, and the call still
    // runs (efrd moves it away before it makes the dir).
    fs.link(&outputs.to_string_lossy(), PROJECT);
    assert!(!floors(&plan(&spec(), &fs)).contains(&output_floor));
    fs.remove(&outputs.to_string_lossy()).dir(&outputs.to_string_lossy());
    assert!(floors(&plan(&spec(), &fs)).contains(&output_floor));
    // Every contained call gets it, a job too.
    let job = job_spec(&mut fs);
    assert!(floors(&plan(&job, &fs)).contains(&output_floor));
}

#[test]
fn job_plan_equals_call_plan_but_the_job_floors() {
    let mut fs = world();
    fs.dir("/home/u/p/app/vendor/lib/.git/hooks").file("/home/u/p/app/vendor/lib/.git/config", "");
    let call = plan(&spec(), &fs);
    let job_spec = job_spec(&mut fs);
    let job = plan(&job_spec, &fs);
    // The same masks, roots, floors and widenings, apart from the job's git dirs.
    assert_eq!(without_job_floors(&job), without_job_floors(&call));
    assert_eq!(job.write_dirs(), call.write_dirs());
    let git_dirs: Vec<PathBuf> = floors(&job)
        .into_iter()
        .filter(|(_, kind)| *kind == FloorKind::JobGitDir)
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        git_dirs,
        [PathBuf::from("/home/u/p/app/.git"), PathBuf::from("/home/u/p/app/vendor/lib/.git")]
    );
    // The pinned git dir stays a mount point, and its floor comes after the pin.
    let at = |path: &str, origin: MountOrigin| {
        job.mounts()
            .iter()
            .position(|mount| mount.op.target() == Path::new(path) && mount.origin == origin)
    };
    let pin = at("/home/u/p/app/.git", MountOrigin::Pin).unwrap();
    let floor = at("/home/u/p/app/.git", MountOrigin::Floor(FloorKind::JobGitDir)).unwrap();
    assert!(pin < floor, "{:?}", job.mounts());
}

/// The seccomp profile that the inner stage installs for `plan`.
fn profile(plan: &MountPlan) -> SeccompProfile {
    InnerPolicy::new(plan, 3, None).seccomp
}

#[test]
fn netview_plan_keeps_write_roots() {
    let mut spec = spec();
    let mut fs = world();
    fs.dir("/home/u/p/out").dir("/home/u/.secret-notes");
    spec.masks.push(Mask { path: "/home/u/.secret-notes".into(), kind: MaskKind::SandboxMask });
    spec.grants = vec![
        Grant::NetView,
        Grant::Write { path: "/home/u/p/out".into() },
        Grant::Unmask { path: "/home/u/.secret-notes".into() },
    ];
    let viewed = plan(&spec, &fs);
    assert!(viewed.net_view());
    assert!(!viewed.unshares_network());
    assert_eq!(profile(&viewed), SeccompProfile::net_view());
    // The view changes only the network: the same mounts and writes as without it.
    spec.grants.retain(|grant| *grant != Grant::NetView);
    let contained = plan(&spec, &fs);
    assert!(!contained.net_view());
    assert!(contained.unshares_network());
    assert_eq!(profile(&contained), SeccompProfile::phase1());
    assert_eq!(viewed.mounts(), contained.mounts());
    assert_eq!(viewed.write_dirs(), contained.write_dirs());
    assert!(viewed.write_dirs().contains(&PathBuf::from("/home/u/p/out")));
    assert!(viewed.write_dirs().contains(&PathBuf::from(PROJECT)));
    assert!(!has_mount(
        &viewed,
        &MountOp::Tmpfs { target: "/home/u/.secret-notes".into(), perms: 0o500 }
    ));
}

#[test]
fn netview_with_open_network_uses_phase1_profile() {
    let mut spec = spec();
    spec.grants = vec![Grant::NetView];
    spec.network = NetworkPlan::Open;
    let plan = plan(&spec, &world());
    assert!(!plan.unshares_network());
    assert!(!plan.net_view());
    assert_eq!(profile(&plan), SeccompProfile::phase1());
    // An `OpenNetwork` grant beside the view fails the plan
    // (`netview_beside_another_grant_fails_the_plan`).
}

/// A proxy plan with the call's own socket, `$CALL/proxy.sock`.
fn proxy() -> NetworkPlan {
    NetworkPlan::Proxy { socket: crate::testing::runtime().proxy_socket() }
}

/// [`world`] with efrd's socket of the call in its call dir.
fn proxy_world() -> FakeFs {
    let mut fs = world();
    fs.socket(&crate::testing::runtime().proxy_socket().to_string_lossy());
    fs
}

#[test]
fn netview_with_proxy_plan_keeps_unshare_net() {
    let mut spec = spec();
    spec.grants = vec![Grant::NetView];
    spec.network = proxy();
    let plan = plan(&spec, &proxy_world());
    assert!(plan.unshares_network());
    assert!(!plan.net_view());
    assert_eq!(profile(&plan), SeccompProfile::phase1());
}

#[test]
fn host_namespace_implies_net_view_or_open() {
    let grants = [
        Grant::NetView,
        Grant::OpenNetwork,
        Grant::Write { path: "/home/u/p/out".into() },
        Grant::Host { host: "example.com".to_owned(), port: 443 },
        Grant::Bus { bus: BusKind::Session },
        Grant::Device { path: "/dev/nvme0n1".into() },
        Grant::Unmask { path: "/home/u/.secret-notes".into() },
    ];
    let mut fs = proxy_world();
    fs.dir("/home/u/p/out").dir("/home/u/.secret-notes");
    let mut built = 0;
    let mut refused = 0;
    let mut proxied = 0;
    for network in [NetworkPlan::None, NetworkPlan::Open, proxy()] {
        for set in 0_u32..(1 << grants.len()) {
            let mut spec = spec();
            spec.masks
                .push(Mask { path: "/home/u/.secret-notes".into(), kind: MaskKind::SandboxMask });
            spec.network = network.clone();
            spec.grants = grants
                .iter()
                .enumerate()
                .filter(|(at, _)| set & (1 << at) != 0)
                .map(|(_, grant)| grant.clone())
                .collect();
            let mixed = spec.grants.contains(&Grant::NetView)
                && spec.grants.iter().any(|grant| {
                    !matches!(grant, Grant::NetView | Grant::Write { .. } | Grant::Unmask { .. })
                });
            let built_plan = MountPlan::build(&spec, &fs);
            let proxy_open = network == proxy() && spec.grants.contains(&Grant::OpenNetwork);
            if proxy_open {
                let what = format!("{network:?} {:?}", spec.grants);
                assert!(matches!(built_plan, Err(SandboxError::ProxyWithOpenNetwork)), "{what}");
                proxied += 1;
                continue;
            }
            if mixed {
                let what = format!("{network:?} {:?}", spec.grants);
                assert!(matches!(built_plan, Err(SandboxError::NetViewMixed)), "{what}");
                refused += 1;
                continue;
            }
            let plan = built_plan.unwrap();
            built += 1;
            let has = |wanted: &Grant| spec.grants.contains(wanted);
            let open = network == NetworkPlan::Open || has(&Grant::OpenNetwork);
            let what = format!("{network:?} {:?}", spec.grants);
            if !plan.unshares_network() {
                let viewed = plan.net_view() && profile(&plan) == SeccompProfile::net_view();
                assert!(viewed || open, "{what}");
            }
            if plan.proxy_socket().is_some() {
                assert!(plan.unshares_network() && !plan.net_view(), "{what}");
            }
            if plan.net_view() {
                assert!(has(&Grant::NetView) && !open, "{what}");
                assert!(!plan.unshares_network(), "{what}");
                assert_eq!(profile(&plan), SeccompProfile::net_view(), "{what}");
            } else {
                assert_eq!(profile(&plan), SeccompProfile::phase1(), "{what}");
            }
        }
    }
    // NOTE: of the 64 sets with `NetView`, the 4 with only `Write` and `Unmask` beside
    // it build. A proxy plan refuses the 64 sets with `OpenNetwork` first, 32 of them
    // with `NetView`.
    assert_eq!(refused, 2 * 60 + 28);
    assert_eq!(proxied, 64);
    assert_eq!(built + refused + proxied, 3 * (1 << grants.len()));
}

#[test]
fn netview_beside_another_grant_fails_the_plan() {
    let mut fs = world();
    fs.dir("/home/u/p/out");
    let mut spec = spec();
    spec.grants = vec![Grant::NetView, Grant::Device { path: "/dev/net/tun".into() }];
    assert!(matches!(MountPlan::build(&spec, &fs), Err(SandboxError::NetViewMixed)));
    spec.grants = vec![Grant::NetView, Grant::OpenNetwork];
    assert!(matches!(MountPlan::build(&spec, &fs), Err(SandboxError::NetViewMixed)));
    // Beside a write grant, the network view stays.
    spec.grants = vec![Grant::NetView, Grant::Write { path: "/home/u/p/out".into() }];
    let plan = MountPlan::build(&spec, &fs).unwrap();
    assert!(plan.net_view());
}

#[test]
fn unknown_grant_fails_the_plan() {
    let mut spec = spec();
    spec.grants = vec![Grant::NetView, Grant::Unknown];
    assert!(matches!(MountPlan::build(&spec, &world()), Err(SandboxError::UnknownGrant)));
    // A spec on the wire with a grant kind of a newer efr reads as one and fails too.
    let mut json: serde_json::Value =
        serde_json::from_slice(&crate::testing::spec().to_json().unwrap()).unwrap();
    json["grants"] = serde_json::json!([{ "kind": "teleport", "to": "/" }]);
    let read = SandboxSpec::from_json(&serde_json::to_vec(&json).unwrap()).unwrap();
    assert_eq!(read.grants, [Grant::Unknown]);
    assert!(matches!(MountPlan::build(&read, &world()), Err(SandboxError::UnknownGrant)));
}

#[test]
fn proxy_plan_binds_the_socket() {
    let mut spec = spec();
    spec.network = proxy();
    let plan = plan(&spec, &proxy_world());
    let runtime = crate::testing::runtime();
    let inside = PathBuf::from("/run/user/1000/efr-proxy.sock");
    assert_eq!(runtime.inside_proxy_socket(), inside);
    assert_eq!(plan.proxy_socket(), Some(inside.as_path()));
    let binds: Vec<_> =
        plan.mounts().iter().filter(|mount| mount.op.target() == inside.as_path()).collect();
    assert_eq!(binds.len(), 1, "{binds:?}");
    assert_eq!(
        binds[0].op,
        MountOp::Bind { source: runtime.proxy_socket(), target: inside.clone(), writable: false }
    );
    assert_eq!(binds[0].origin, MountOrigin::Asset);
    // The bind lands in the empty runtime dir of the call, after its tmpfs.
    let tmpfs = plan
        .mounts()
        .iter()
        .position(|mount| mount.op.target() == Path::new("/run/user/1000"))
        .unwrap();
    let bind = plan.mounts().iter().position(|mount| mount.op.target() == inside.as_path());
    assert!(bind.is_some_and(|bind| bind > tmpfs), "{:?}", plan.mounts());
    assert_eq!(InnerPolicy::new(&plan, 3, None).proxy_socket, Some(inside));
    // No other call gets the bind or starts a bridge.
    let none = plan_of(&crate::testing::spec(), &proxy_world());
    assert_eq!(none.proxy_socket(), None);
    assert_eq!(InnerPolicy::new(&none, 3, None).proxy_socket, None);
    assert!(!none.mounts().iter().any(|mount| mount.op.target() == runtime.inside_proxy_socket()));
}

#[test]
fn proxy_plan_keeps_unshare_net() {
    let mut spec = spec();
    spec.network = proxy();
    let plan = plan(&spec, &proxy_world());
    assert!(plan.unshares_network());
    assert!(!plan.net_view());
    assert_eq!(profile(&plan), SeccompProfile::phase1());
}

#[test]
fn proxy_plan_child_has_no_socket_rule() {
    let mut spec = spec();
    spec.network = proxy();
    let plan = plan(&spec, &proxy_world());
    let runtime = crate::testing::runtime();
    let policy = plan.landlock();
    assert!(policy.resolve_unix.is_empty(), "{:?}", policy.resolve_unix);
    for rule in policy.rules() {
        assert!(!rule.access.contains(&crate::FsAccess::ResolveUnix), "{rule:?}");
        assert!(!rule.path.starts_with(&runtime.runtime), "{rule:?}");
        assert_ne!(rule.path, runtime.inside_proxy_socket(), "{rule:?}");
    }
    // With a bus grant beside it, the child's one socket rule is the granted bus.
    spec.grants.push(Grant::Bus { bus: BusKind::System });
    let policy = plan_of(&spec, &proxy_world()).landlock();
    assert_eq!(policy.resolve_unix, [PathBuf::from("/run/dbus/system_bus_socket")]);
}

fn plan_of(spec: &SandboxSpec, fs: &FakeFs) -> MountPlan {
    MountPlan::build(spec, fs).unwrap()
}

#[test]
fn proxy_socket_only_with_unshare_net() {
    for network in [NetworkPlan::None, NetworkPlan::Open] {
        let mut spec = spec();
        spec.network = network;
        assert_eq!(plan(&spec, &proxy_world()).proxy_socket(), None);
    }
    let mut spec = spec();
    spec.network = proxy();
    spec.grants = vec![Grant::OpenNetwork];
    assert!(matches!(
        MountPlan::build(&spec, &proxy_world()),
        Err(SandboxError::ProxyWithOpenNetwork)
    ));
    spec.grants = vec![Grant::NetView];
    let plan = plan(&spec, &proxy_world());
    assert!(plan.proxy_socket().is_some() && plan.unshares_network());
}

#[test]
fn proxy_plan_refuses_a_socket_that_is_missing_or_not_a_socket() {
    let mut spec = spec();
    spec.network = proxy();
    assert!(MountPlan::build(&spec, &world()).is_err());
    let mut fs = world();
    fs.file(&crate::testing::runtime().proxy_socket().to_string_lossy(), "not a socket");
    assert!(matches!(MountPlan::build(&spec, &fs), Err(SandboxError::ProxySocket { .. })));
}

#[test]
fn proxy_plan_sets_the_bridge_variables() {
    let mut spec = spec();
    spec.network = proxy();
    let plan = plan(&spec, &proxy_world());
    let env = plan.env_overrides();
    for name in crate::PROXY_VARIABLES {
        assert!(env.contains(&((*name).to_owned(), crate::PROXY_URL.to_owned())), "{name}");
    }
    for name in ["NO_PROXY", "no_proxy"] {
        assert!(env.contains(&(name.to_owned(), crate::NO_PROXY.to_owned())), "{name}");
    }
    assert!(env.contains(&("NODE_USE_ENV_PROXY".to_owned(), "1".to_owned())));
    assert_eq!(crate::PROXY_URL, format!("http://127.0.0.1:{}", crate::PROXY_PORT));
    let none = plan_of(&crate::testing::spec(), &world());
    assert!(none.env_overrides().is_empty(), "{:?}", none.env_overrides());
}

/// The filtered buses of a call: both grants, and the world with the call's sockets.
fn bus_filter_world() -> (SandboxSpec, FakeFs) {
    let mut spec = spec();
    spec.grants =
        vec![Grant::BusFilter { bus: BusKind::System }, Grant::BusFilter { bus: BusKind::Session }];
    let mut fs = world();
    for bus in [BusKind::System, BusKind::Session] {
        fs.socket(&spec.runtime.bus_filter_socket(bus).to_string_lossy());
    }
    (spec, fs)
}

#[test]
fn bus_filter_binds_call_socket_read_only() {
    let (spec, fs) = bus_filter_world();
    let plan = plan(&spec, &fs);
    let call = &spec.runtime.call_dir;
    let binds: Vec<(&MountOp, MountOrigin, OpKind)> = plan
        .mounts()
        .iter()
        .filter(|mount| {
            matches!(&mount.op, MountOp::Bind { source, .. } if source.starts_with(call.join("bus")))
        })
        .map(|mount| (&mount.op, mount.origin, mount.kind))
        .collect();
    let system = MountOp::Bind {
        source: call.join("bus/system.sock"),
        target: "/run/dbus/system_bus_socket".into(),
        writable: false,
    };
    let session = MountOp::Bind {
        source: call.join("bus/session.sock"),
        target: "/run/user/1000/bus".into(),
        writable: false,
    };
    assert_eq!(
        binds,
        [
            (&system, MountOrigin::Asset, OpKind::Floor),
            (&session, MountOrigin::Asset, OpKind::Floor),
        ]
    );
    // The paths of the contract.
    assert_eq!(spec.runtime.bus_filter_dir(), call.join("bus"));
    assert_eq!(crate::bus_filter_file(BusKind::System), "system.sock");
    assert_eq!(crate::bus_filter_file(BusKind::Session), "session.sock");
    assert_eq!(spec.runtime.inside_bus_socket(BusKind::Session), Path::new("/run/user/1000/bus"));
    // The session bus lands in the runtime dir's tmpfs, after it; the real buses of the
    // host are never bound.
    let at = |target: &str| {
        plan.mounts().iter().position(|mount| mount.op.target() == Path::new(target)).unwrap()
    };
    assert!(at("/run/user/1000") < at("/run/user/1000/bus"));
    assert!(!has_mount(&plan, &rw("/run/user/1000/bus")));
    assert!(!has_mount(&plan, &rw("/run/dbus/system_bus_socket")));
    assert!(plan.unshares_network());
    // A bus named twice binds once.
    let mut twice = spec.clone();
    twice.grants.push(Grant::BusFilter { bus: BusKind::System });
    assert_eq!(plan_of(&twice, &fs).mounts(), plan.mounts());
}

#[test]
fn bus_filter_resolve_unix_holds_only_the_target() {
    let (spec, fs) = bus_filter_world();
    let policy = plan(&spec, &fs).landlock();
    assert_eq!(
        policy.resolve_unix,
        [PathBuf::from("/run/dbus/system_bus_socket"), PathBuf::from("/run/user/1000/bus")]
    );
    assert!(
        !policy.resolve_unix.iter().any(|path| path.starts_with(&spec.runtime.runtime)),
        "no rule below efr's runtime root: {:?}",
        policy.resolve_unix
    );
    // One bus, one rule.
    let mut one = spec.clone();
    one.grants = vec![Grant::BusFilter { bus: BusKind::Session }];
    assert_eq!(plan_of(&one, &fs).landlock().resolve_unix, [PathBuf::from("/run/user/1000/bus")]);
    // No view, no rule.
    assert!(plan_of(&crate::testing::spec(), &fs).landlock().resolve_unix.is_empty());
}

#[test]
fn bus_filter_sets_the_bus_addresses() {
    let (spec, fs) = bus_filter_world();
    let plan = plan(&spec, &fs);
    assert_eq!(
        plan.env_overrides(),
        [
            (
                "DBUS_SYSTEM_BUS_ADDRESS".to_owned(),
                "unix:path=/run/dbus/system_bus_socket".to_owned()
            ),
            ("DBUS_SESSION_BUS_ADDRESS".to_owned(), "unix:path=/run/user/1000/bus".to_owned()),
        ]
    );
    // The address of the trusted shell never reaches the call: the plan's wins.
    let filter = crate::EnvFilter::new(&spec, &plan);
    let host: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> =
        [("DBUS_SESSION_BUS_ADDRESS".into(), "unix:path=/run/user/1000/bus,guid=1".into())]
            .into_iter()
            .collect();
    let (env, _) = filter.apply(&host);
    assert_eq!(
        env.get(std::ffi::OsStr::new("DBUS_SESSION_BUS_ADDRESS")),
        Some(&std::ffi::OsString::from("unix:path=/run/user/1000/bus"))
    );
}

#[test]
fn bus_filter_beside_real_bus_refused() {
    let (mut spec, fs) = bus_filter_world();
    spec.grants.push(Grant::Bus { bus: BusKind::Session });
    let error = MountPlan::build(&spec, &fs).unwrap_err();
    assert!(matches!(error, SandboxError::BusFilterWithBus { bus: BusKind::Session }), "{error:?}");
    assert!(matches!(spec.check(), Err(SandboxError::BusFilterWithBus { .. })));
    // The filter of one bus beside the whole other bus is a plan of both.
    let mut spec = crate::testing::spec();
    spec.grants =
        vec![Grant::BusFilter { bus: BusKind::System }, Grant::Bus { bus: BusKind::Session }];
    let both = plan(&spec, &fs);
    assert!(has_mount(&both, &rw("/run/user/1000/bus")));
    let filtered = MountOp::Bind {
        source: spec.runtime.bus_filter_socket(BusKind::System),
        target: "/run/dbus/system_bus_socket".into(),
        writable: false,
    };
    assert!(has_mount(&both, &filtered));
    // The network view never joins a filtered bus (DE2).
    let mut spec = crate::testing::spec();
    spec.grants = vec![Grant::NetView, Grant::BusFilter { bus: BusKind::System }];
    assert!(matches!(MountPlan::build(&spec, &fs), Err(SandboxError::NetViewMixed)));
}

#[test]
fn bus_filter_masks_run_dbus() {
    let (mut spec, fs) = bus_filter_world();
    spec.grants = vec![Grant::BusFilter { bus: BusKind::System }];
    let plan = plan(&spec, &fs);
    let tmpfs = MountOp::Tmpfs { target: "/run/dbus".into(), perms: 0o755 };
    let bind = MountOp::Bind {
        source: spec.runtime.bus_filter_socket(BusKind::System),
        target: "/run/dbus/system_bus_socket".into(),
        writable: false,
    };
    let at = |op: &MountOp| plan.mounts().iter().position(|mount| mount.op == *op).unwrap();
    assert!(at(&tmpfs) < at(&bind), "{:#?}", plan.mounts());
    // The rest of /run/dbus is hidden; the socket there is the call's own, read-only.
    assert_eq!(plan.explain(Path::new("/run/dbus/other")).role, SandboxPathRole::Masked);
    let socket = plan.explain(Path::new("/run/dbus/system_bus_socket"));
    assert_eq!((socket.role, socket.write), (SandboxPathRole::Floor, false));
    // The session filter alone leaves /run/dbus as it is.
    spec.grants = vec![Grant::BusFilter { bus: BusKind::Session }];
    assert!(!has_mount(&plan_of(&spec, &fs), &tmpfs));
}

#[test]
fn bus_filter_without_run_dbus_fails() {
    let (mut spec, mut fs) = bus_filter_world();
    fs.remove("/run/dbus");
    let error = MountPlan::build(&spec, &fs).unwrap_err();
    assert!(matches!(error, SandboxError::NoSystemBus), "{error:?}");
    // A link in place of the dir fails the same way.
    fs.dir("/run/other").link("/run/dbus", "/run/other");
    assert!(matches!(MountPlan::build(&spec, &fs), Err(SandboxError::NoSystemBus)));
    // The session filter needs no /run/dbus.
    spec.grants = vec![Grant::BusFilter { bus: BusKind::Session }];
    assert!(MountPlan::build(&spec, &fs).is_ok());
    // A call socket that efrd did not make, or that is not a socket, fails closed.
    let (spec, mut fs) = bus_filter_world();
    let session = spec.runtime.bus_filter_socket(BusKind::Session);
    fs.remove(&session.to_string_lossy());
    let error = MountPlan::build(&spec, &fs).unwrap_err();
    assert!(matches!(&error, SandboxError::GrantTarget { path } if *path == session), "{error:?}");
    fs.file(&session.to_string_lossy(), "not a socket");
    assert!(matches!(MountPlan::build(&spec, &fs), Err(SandboxError::GrantTarget { .. })));
}

#[test]
fn always_allow_under_dir_keeps_floor_binds() {
    // A write grant of `~/dotfiles`, the target of the `~/.zshrc` link: the floor's real
    // file stays read-only after the widening.
    let mut spec = spec();
    spec.grants = vec![Grant::Write { path: "/home/u/dotfiles".into() }];
    let mut fs = world();
    fs.remove("/home/u/.zshrc")
        .file("/home/u/dotfiles/zsh/.zshrc", "x")
        .link("/home/u/.zshrc", "/home/u/dotfiles/zsh/.zshrc");
    let plan = plan(&spec, &fs);
    assert!(has_mount(&plan, &rw("/home/u/dotfiles")));
    assert!(
        floors(&plan).contains(&("/home/u/dotfiles/zsh/.zshrc".into(), FloorKind::ShellStartup)),
        "{:#?}",
        plan.mounts()
    );
    let at = |op: &MountOp| plan.mounts().iter().position(|mount| mount.op == *op).unwrap();
    assert!(at(&rw("/home/u/dotfiles")) < at(&ro("/home/u/dotfiles/zsh/.zshrc")));
    // A floor that does not exist below a dir grant gets no bind, only a note; the
    // engine refuses an always rule for such a dir (DE20 (b)).
    let mut spec = crate::testing::spec();
    spec.grants = vec![Grant::Write { path: "/home/u/.config".into() }];
    spec.floors
        .push(Floor { path: "/home/u/.config/autostart".into(), kind: FloorKind::Autostart });
    let missing = plan_of(&spec, &world());
    assert!(missing.notes().contains(&PlanNote::MissingFloor("/home/u/.config/autostart".into())));
    assert!(!floors(&missing).iter().any(|(path, _)| path.ends_with("autostart")));
}
