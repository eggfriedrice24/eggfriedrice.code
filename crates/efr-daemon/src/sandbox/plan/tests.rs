//! One call's spec from plain paths.

use std::path::{Path, PathBuf};

use efr_config::{SandboxSettings, WriteProjects};
use efr_protocol::{CacheMode, CallId, ConversationId, Grant, Launch};
use efr_sandbox::{FloorKind, MaskKind, NetworkPlan, SpecLaunch, WriteRootKind};
use efr_stdx::paths::Dirs;

use super::{HostFacts, PlanInput, Planned, build, env_files, floor_kind};
use crate::sandbox::projects::{WORKTREE_CHANGED, register};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    dirs: Dirs,
    host: HostFacts,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        let dirs =
            Dirs::new(root.join("c"), root.join("d"), root.join("s"), root.join("r")).unwrap();
        for made in [&home, &home.join("p/app"), &home.join("p/lib"), &dirs.state().to_path_buf()] {
            std::fs::create_dir_all(made).unwrap();
        }
        let host = HostFacts {
            path: format!("/usr/bin:{}", home.join("p/app/bin").display()),
            user_runtime: root.join("xrt"),
            ..HostFacts::default()
        };
        Fixture { _dir: dir, root, home, dirs, host }
    }

    fn plan(
        &self,
        settings: &SandboxSettings,
        launch: &Launch,
        turn: Option<&Path>,
        named: &[PathBuf],
    ) -> Planned {
        let projects = [self.home.join("p/app"), self.home.join("p/lib"), self.home.clone()];
        let input = PlanInput {
            conversation: ConversationId::from_uuid(uuid::Uuid::from_u128(1)),
            call: CallId::from_uuid(uuid::Uuid::from_u128(2)),
            launch,
            turn_project: turn,
            projects: &projects,
            named_paths: named,
            scratch: &self.dirs.data().join("scratch/x"),
            settings,
            secrets: &[self.home.join(".ssh")],
            protected_config: &[self.dirs.config().to_path_buf()],
            host: &self.host,
            dirs: &self.dirs,
            home: &self.home,
            bwrap: Path::new("/usr/bin/bwrap"),
            cache_mode: CacheMode::Tmp,
            launcher: &self.dirs.runtime().join("bin/efr-sbx"),
        };
        build(&input)
    }
}

fn roots(planned: &Planned) -> Vec<(PathBuf, WriteRootKind)> {
    planned.spec.write_roots.iter().map(|root| (root.path.clone(), root.kind)).collect()
}

#[test]
fn the_turn_project_and_the_named_ones_are_roots_but_never_home() {
    let fixture = Fixture::new();
    let home = &fixture.home;
    let settings = SandboxSettings::default();
    let named = [home.join("p/lib/src/x.rs")];
    let planned = fixture.plan(&settings, &Launch::contained(), Some(home), &named);
    assert_eq!(roots(&planned), [(home.join("p/lib"), WriteRootKind::NamedProject)]);
    let planned = fixture.plan(&settings, &Launch::contained(), Some(&home.join("p/app")), &[]);
    assert_eq!(roots(&planned), [(home.join("p/app"), WriteRootKind::TurnProject)]);
    let mut all = settings.clone();
    all.write_projects = WriteProjects::All;
    let planned = fixture.plan(&all, &Launch::contained(), None, &[]);
    assert_eq!(
        roots(&planned),
        [
            (home.join("p/app"), WriteRootKind::NamedProject),
            (home.join("p/lib"), WriteRootKind::NamedProject),
        ]
    );
    let mut turn = settings;
    turn.write_projects = WriteProjects::Turn;
    let planned = fixture.plan(&turn, &Launch::contained(), None, &named);
    assert!(roots(&planned).is_empty());
}

#[test]
fn worktree_record_mismatch_drops_git_roots() {
    let fixture = Fixture::new();
    let home = &fixture.home;
    let main = home.join("p/lib/.git/worktrees/wt");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(main.join("commondir"), "../..\n").unwrap();
    let wt = home.join("p/app");
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", main.display())).unwrap();
    register(fixture.dirs.state(), &wt).unwrap().unwrap();
    let mut settings = SandboxSettings::default();
    settings.write_projects = WriteProjects::Turn;
    let planned = fixture.plan(&settings, &Launch::contained(), Some(&wt), &[]);
    assert_eq!(
        roots(&planned),
        [
            (wt.clone(), WriteRootKind::TurnProject),
            (main.clone(), WriteRootKind::GitDir),
            (home.join("p/lib/.git"), WriteRootKind::GitCommonDir),
        ]
    );
    assert_eq!(planned.spec.git_dirs, [main.clone(), home.join("p/lib/.git")]);
    assert!(planned.notes.is_empty());
    // A call rewrote the .git file: the git dirs are no roots any more.
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", home.join("p/lib").display())).unwrap();
    let planned = fixture.plan(&settings, &Launch::contained(), Some(&wt), &[]);
    assert_eq!(roots(&planned), [(wt, WriteRootKind::TurnProject)]);
    assert!(planned.spec.git_dirs.is_empty());
    assert_eq!(planned.notes, [WORKTREE_CHANGED]);
}

#[test]
fn masks_floors_grants_and_the_runtime_follow_the_inputs() {
    let fixture = Fixture::new();
    let home = &fixture.home;
    let app = home.join("p/app");
    std::fs::write(app.join(".env"), "SECRET=1").unwrap();
    std::fs::write(app.join(".env.example"), "SECRET=").unwrap();
    std::fs::create_dir_all(app.join("node_modules/x")).unwrap();
    std::fs::write(app.join("node_modules/x/.env"), "").unwrap();
    let mut settings = SandboxSettings::default();
    settings.mask = vec!["~/private".into()];
    settings.protect = vec!["~/bin".into()];
    settings.write_roots = vec!["~/notes".into(), "~".into()];
    let launch = Launch::Contained {
        grants: vec![Grant::OpenNetwork, Grant::Write { path: home.join("out") }],
    };
    let planned = fixture.plan(&settings, &launch, Some(&app), &[]);
    let spec = &planned.spec;
    let mask =
        |path: PathBuf| spec.masks.iter().find(|mask| mask.path == path).map(|mask| mask.kind);
    assert_eq!(mask(home.join(".ssh")), Some(MaskKind::EngineSecret));
    assert_eq!(mask(home.join(".aws")), Some(MaskKind::SandboxMask));
    assert_eq!(mask(home.join("private")), Some(MaskKind::User));
    assert_eq!(mask(app.join(".env")), Some(MaskKind::ProjectEnv));
    assert_eq!(mask(app.join(".env.example")), None);
    assert_eq!(mask(app.join("node_modules/x/.env")), None);
    let floor =
        |path: PathBuf| spec.floors.iter().find(|floor| floor.path == path).map(|floor| floor.kind);
    assert_eq!(floor(fixture.dirs.config().to_path_buf()), Some(FloorKind::Config));
    assert_eq!(floor(home.join(".zshrc")), Some(FloorKind::ShellStartup));
    assert_eq!(floor(home.join(".config/systemd")), Some(FloorKind::Autostart));
    assert_eq!(floor(home.join(".gitconfig")), Some(FloorKind::ToolConfig));
    assert_eq!(floor(app.join(".envrc")), Some(FloorKind::ProtectedName));
    assert_eq!(floor(app.join(".claude")), Some(FloorKind::ProtectedName));
    assert_eq!(floor(home.join("bin")), Some(FloorKind::User));
    assert!(roots(&planned).contains(&(home.join("notes"), WriteRootKind::UserConfigured)));
    assert!(!roots(&planned).iter().any(|(path, _)| path == home), "home is never a root");
    assert_eq!(spec.network, NetworkPlan::Open);
    assert_eq!(spec.grants.len(), 2);
    assert_eq!(spec.launch, SpecLaunch::Contained);
    assert_eq!(spec.guard_roots.last(), Some(&fixture.dirs.data().join("scratch/x")));
    assert_eq!(spec.runtime.call_dir, spec.runtime.shell_dir.join(spec.call.to_string()));
    assert_eq!(spec.runtime.child_script, fixture.dirs.runtime().join("zsh/efr-child.zsh"));
    assert_eq!(spec.runtime.user_runtime, fixture.root.join("xrt"));
    assert_eq!(spec.shell_path, fixture.host.path);
    spec.check().unwrap();
    let unsandboxed = fixture.plan(&settings, &Launch::Unsandboxed, Some(&app), &[]);
    assert_eq!(unsandboxed.spec.launch, SpecLaunch::Unsandboxed);
    assert_eq!(unsandboxed.spec.network, NetworkPlan::None);
}

#[test]
fn env_globs_keep_their_exceptions_and_skip_build_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("a/b/c")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    for file in [".env", ".env.local", ".env.sample", "a/.env", "a/b/c/.env", "target/.env"] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let globs = SandboxSettings::default().mask_globs;
    let found = env_files(root, &globs);
    assert_eq!(found, [root.join(".env"), root.join(".env.local"), root.join("a/.env")]);
}

#[test]
fn persistence_floors_have_their_kind() {
    assert_eq!(floor_kind(Path::new(".bash_profile")), FloorKind::ShellStartup);
    assert_eq!(floor_kind(Path::new(".config/fish")), FloorKind::ShellStartup);
    assert_eq!(floor_kind(Path::new(".config/hypr")), FloorKind::Autostart);
    assert_eq!(floor_kind(Path::new(".cargo/bin")), FloorKind::ToolConfig);
}
