use std::path::{Path, PathBuf};

use efr_config::Settings;
use efr_permissions::{
    Action, CommandPattern, ConversationPolicy, DecisionInput, Effect, Engine, PathClass, Policy,
    Rebuild, Requirements, Resource, Rule,
};
use efr_protocol::{Mode, Origin, ProjectId, Scope};
use efr_scope::{Home, Registry};
use pretty_assertions::assert_eq;

use crate::engine::{EngineParts, Probed, build, load_registry, protected_config};

fn parts(home: &Home, secrets: PathBuf) -> EngineParts {
    EngineParts {
        home: home.clone(),
        secrets,
        registry: home.path().join(".config/efr/projects.toml"),
        config: home.path().join(".config/efr"),
        host: crate::sandbox::HostFacts::default(),
        runtime: home.path().join(".run/efr"),
        data: home.path().join(".local/share/efr"),
        state: home.path().join(".local/state/efr"),
        launcher: Some(home.path().join(".local/lib/efr/efr-sbx")),
    }
}

/// A fresh home directory in its resolved form.
fn home() -> (tempfile::TempDir, Home) {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    (root, Home::new(&home).unwrap())
}

fn decide(engine: &Engine, home: &Home, mode: Mode, requirements: Requirements) -> Effect {
    let input = DecisionInput {
        requirements,
        scope: Scope::Machine,
        origin: Origin::Shell,
        mode,
        conversation_policy: ConversationPolicy::new(home.path().join("scratch")),
    };
    engine.decide(&input).effect()
}

#[tokio::test]
async fn the_secret_paths_of_the_config_classify_as_secrets() {
    let (_root, home) = home();
    let mut settings = Settings::default();
    settings.permissions.secret_paths =
        vec![PathBuf::from("~/.config/rclone/rclone.conf"), PathBuf::from("/srv/vault")];

    let engine = parts(&home, home.path().join("secrets"))
        .engine(&settings, Probed::default())
        .await
        .unwrap();

    let scratch = home.path().join("scratch");
    let class = |path: PathBuf| engine.locations().classify(&path, &scratch);
    assert_eq!(class(home.path().join(".config/rclone/rclone.conf")), Some(PathClass::Secrets));
    assert_eq!(class(PathBuf::from("/srv/vault/token")), Some(PathClass::Secrets));
    assert_eq!(class(home.path().join(".config/rclone/other.conf")), Some(PathClass::UserConfig));
}

#[tokio::test]
async fn each_mode_decides_by_its_built_in_rules_then_the_users() {
    let (_root, home) = home();
    let rule = Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("cargo").with_args(["test"])),
        Effect::Allow,
    );
    let mut settings = Settings::default();
    settings.permissions.rules = Policy::new(vec![rule.clone()]).unwrap();

    let engine = parts(&home, home.path().join("secrets"))
        .engine(&settings, Probed::default())
        .await
        .unwrap();

    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        let base = Policy::base(mode);
        let rules = engine.policy(mode).rules();
        assert_eq!(&rules[..base.rules().len()], base.rules(), "{mode}");
        assert_eq!(rules[base.rules().len()..], *std::slice::from_ref(&rule), "{mode}");
    }
}

#[tokio::test]
async fn no_rule_of_the_users_opens_the_daemons_own_secrets() {
    let (_root, home) = home();
    let every_secret = Rule::new(Action::Read, Resource::Class(PathClass::Secrets), Effect::Allow);
    let mut settings = Settings::default();
    settings.permissions.rules = Policy::new(vec![every_secret]).unwrap();
    let secrets = home.path().join(".local/share/efr/secrets");

    let engine = parts(&home, secrets.clone()).engine(&settings, Probed::default()).await.unwrap();

    let read = |path: PathBuf| Requirements::none().with_read(path);
    let key = read(home.path().join(".ssh/id_ed25519"));
    assert_eq!(decide(&engine, &home, Mode::Cautious, key), Effect::Allow);
    let token = read(secrets.join("openai-subscription.json"));
    assert_eq!(decide(&engine, &home, Mode::Cautious, token), Effect::Deny);
}

#[tokio::test]
async fn no_tool_writes_efrs_own_programs_and_roots_but_scratch_and_trash_stay_open() {
    let (_root, home) = home();
    let mut parts = parts(&home, home.path().join(".local/share/efr/secrets"));
    parts.host.binaries = vec![home.path().join(".local/bin/efrd")];
    let engine = parts.engine(&Settings::default(), Probed::default()).await.unwrap();

    let write = |path: PathBuf| Requirements::none().with_write(path);
    let data = home.path().join(".local/share/efr");
    for path in [
        data.join("efr.sqlite"),
        data.join("file-backups/x"),
        home.path().join(".run/efr/zsh/.zshrc"),
        home.path().join(".run/efr/bin/efr-sbx"),
        home.path().join(".local/state/efr/logs/x"),
        home.path().join(".local/lib/efr/efr-sbx"),
        home.path().join(".local/bin/efrd"),
    ] {
        for mode in [Mode::Auto, Mode::Cautious] {
            assert_eq!(decide(&engine, &home, mode, write(path.clone())), Effect::Deny, "{path:?}");
        }
    }
    for path in [data.join("scratch/x/notes.txt"), data.join("trash/x")] {
        assert_ne!(
            decide(&engine, &home, Mode::Auto, write(path.clone())),
            Effect::Deny,
            "{path:?}"
        );
    }
}

#[tokio::test]
async fn only_the_user_approves_the_socket_of_the_environments_ssh_agent() {
    let (_root, home) = home();
    let mut parts = parts(&home, home.path().join("secrets"));
    let env = [
        ("SSH_AUTH_SOCK".to_owned(), "/run/user/1000/vault.sock".to_owned()),
        ("GPG_AGENT_INFO".to_owned(), "/run/user/1000/pgp.sock:0:1".to_owned()),
    ];
    parts.host = crate::sandbox::HostFacts::from_env(&env.into_iter().collect(), None);
    let engine = parts.engine(&Settings::default(), Probed::default()).await.unwrap();
    for (socket, user_only) in [
        ("/run/user/1000/vault.sock", true),
        ("/run/user/1000/pgp.sock", true),
        ("/run/user/1000/app.sock", false),
    ] {
        let needs = efr_protocol::Needs { sockets: vec![socket.to_owned()], ..Default::default() };
        let input = DecisionInput {
            requirements: Requirements::none()
                .with_command("ssh host x")
                .with_command_dir(home.path())
                .with_needs(needs),
            scope: Scope::Machine,
            origin: Origin::Shell,
            mode: Mode::Auto,
            conversation_policy: ConversationPolicy::new(home.path().join("scratch")),
        };
        let decision = engine.decide(&input);
        let need = decision
            .exits()
            .find(|need| need.kind == efr_protocol::ExitKind::Socket)
            .unwrap_or_else(|| panic!("{socket}: no socket exit"));
        assert_eq!(need.user_only(), user_only, "{socket}");
    }
}

#[tokio::test]
async fn the_engine_marks_caches_and_efrs_trash_as_rebuildable() {
    let (_root, home) = home();
    let engine = parts(&home, home.path().join("secrets"))
        .engine(&Settings::default(), Probed::default())
        .await
        .unwrap();
    let rebuild = |path: PathBuf| {
        engine.path_facts(&path, &Scope::Machine, &home.path().join("scratch")).unwrap().rebuild
    };
    assert_eq!(rebuild(home.path().join(".cache/pip/x")), Some(Rebuild::Cache));
    assert_eq!(rebuild(home.path().join(".local/share/efr/trash/x")), Some(Rebuild::Trash));
    assert_eq!(rebuild(home.path().join("Documents/build/x")), None);
}

#[tokio::test]
async fn a_registered_project_is_writable_and_a_missing_registry_registers_none() {
    let (_root, home) = home();
    let app = home.path().join("p/app");
    std::fs::create_dir_all(&app).unwrap();
    let id = ProjectId::from_uuid(uuid::Uuid::from_u128(7));
    let registry_path = home.path().join("projects.toml");
    assert!(load_registry(&registry_path).await.projects().is_empty());
    let mut registry = Registry::empty();
    registry.register(id, &app, None).unwrap();
    registry.save(&registry_path).unwrap();

    let projects = load_registry(&registry_path).await;
    let engine = build(
        &home,
        &home.path().join("secrets"),
        &[],
        &Settings::default(),
        &projects,
        &crate::engine::SandboxFacts::default(),
        Probed::default(),
    )
    .unwrap();

    let input = DecisionInput {
        requirements: Requirements::none().with_write(app.join("src/main.rs")),
        scope: Scope::Project(id),
        origin: Origin::Shell,
        mode: Mode::Cautious,
        conversation_policy: ConversationPolicy::new(home.path().join("scratch")),
    };
    assert_eq!(engine.decide(&input).effect(), Effect::Allow);
}

#[test]
fn the_protected_config_is_the_directory_and_what_its_links_reach() {
    let (_root, home) = home();
    let config = home.path().join(".config/efr");
    let dotfiles = home.path().join("dotfiles/efr");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&dotfiles).unwrap();
    std::fs::write(dotfiles.join("config.toml"), "").unwrap();
    std::fs::write(config.join("projects.toml"), "").unwrap();
    std::os::unix::fs::symlink(dotfiles.join("config.toml"), config.join("config.toml")).unwrap();
    std::os::unix::fs::symlink("../../dotfiles/efr/themes", config.join("themes")).unwrap();

    let mut protected = protected_config(&config);

    assert_eq!(protected[0], config, "the directory comes first");
    protected.sort();
    let mut expected = vec![
        config.clone(),
        dotfiles.join("config.toml"),
        // A link whose target is missing: a write through it creates the target.
        config.join("../../dotfiles/efr/themes"),
    ];
    expected.sort();
    assert_eq!(protected, expected, "a plain file such as projects.toml is in the directory");
}

#[test]
fn a_linked_config_directory_is_protected_in_both_forms_and_a_missing_one_alone() {
    let (_root, home) = home();
    let real = home.path().join("dotfiles/efr");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(home.path().join(".config")).unwrap();
    let config = home.path().join(".config/efr");
    std::os::unix::fs::symlink(&real, &config).unwrap();

    assert_eq!(protected_config(&config), [config.clone(), real]);
    let missing = home.path().join(".config/other");
    assert_eq!(protected_config(&missing), [missing]);
}

#[tokio::test]
async fn no_tool_writes_the_config_or_the_file_behind_its_link_in_any_mode() {
    let (_root, home) = home();
    let config = home.path().join(".config/efr");
    let dotfiles = home.path().join("dotfiles/efr");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&dotfiles).unwrap();
    std::fs::write(dotfiles.join("config.toml"), "").unwrap();
    std::os::unix::fs::symlink(dotfiles.join("config.toml"), config.join("config.toml")).unwrap();
    // A rule of the user's that would open all of `~` for writing does not open it.
    let mut settings = Settings::default();
    settings.permissions.rules =
        Policy::new(vec![Rule::new(Action::Write, Resource::Under("~".into()), Effect::Allow)])
            .unwrap();

    let engine = parts(&home, home.path().join("secrets"))
        .engine(&settings, Probed::default())
        .await
        .unwrap();

    let write = |path: &Path| Requirements::none().with_write(path);
    for mode in [Mode::Manual, Mode::Cautious, Mode::Auto] {
        for path in
            [config.join("config.toml"), config.join("projects.toml"), dotfiles.join("config.toml")]
        {
            assert_eq!(decide(&engine, &home, mode, write(&path)), Effect::Deny, "{path:?}");
        }
        let other = home.path().join("dotfiles/zshrc");
        assert_eq!(decide(&engine, &home, mode, write(&other)), Effect::Allow, "{mode}");
    }
    let read = Requirements::none().with_read(config.join("config.toml"));
    assert_eq!(decide(&engine, &home, Mode::Cautious, read), Effect::Allow);
}

#[tokio::test]
async fn the_file_that_efr_config_set_writes_is_sealed_and_the_writer_still_writes_it() {
    let (_root, home) = home();
    let config = home.path().join(".config/efr");
    let dotfiles = home.path().join("dotfiles/efr");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&dotfiles).unwrap();
    std::fs::write(dotfiles.join("config.toml"), "# mine\n[model]\nname = \"gpt-5.5\"\n").unwrap();
    std::os::unix::fs::symlink(dotfiles.join("config.toml"), config.join("config.toml")).unwrap();
    let other = home.path().join("other/efr");
    std::fs::create_dir_all(&other).unwrap();

    // The writer of `efr config set` is not a tool: protection does not stop it.
    let linked = efr_config::ConfigFile::open(&config.join("config.toml")).unwrap();
    let mut edit = linked.edit().unwrap();
    edit.set_text("model.effort", "high").unwrap();
    linked.write(&edit).unwrap();
    let missing = efr_config::ConfigFile::open(&other.join("config.toml")).unwrap();
    let mut edit = missing.edit().unwrap();
    edit.set_text("model.effort", "low").unwrap();
    missing.write(&edit).unwrap();

    let written = std::fs::read_to_string(dotfiles.join("config.toml")).unwrap();
    assert!(written.starts_with("# mine\n") && written.contains("effort = \"high\""), "{written}");
    assert!(std::fs::symlink_metadata(config.join("config.toml")).unwrap().is_symlink());
    assert_eq!(linked.target(), dotfiles.join("config.toml"));
    // The engine protects what the writer wrote, behind the link or created new.
    let settings = Settings::default();
    let engine = parts(&home, home.path().join("secrets"))
        .engine(&settings, Probed::default())
        .await
        .unwrap();
    let mut created = parts(&home, home.path().join("secrets"));
    created.config.clone_from(&other);
    let created = created.engine(&settings, Probed::default()).await.unwrap();
    for mode in Mode::ALL {
        for path in [linked.path(), linked.target()] {
            let write = Requirements::none().with_write(path);
            assert_eq!(decide(&engine, &home, mode, write), Effect::Deny, "{mode} {path:?}");
        }
        let write = Requirements::none().with_write(missing.target());
        assert_eq!(decide(&created, &home, mode, write), Effect::Deny, "{mode}");
    }
}

#[tokio::test]
async fn the_auto_support_follows_snapshot_undo_and_ask_destructive() {
    let (_root, home) = home();
    let support = async |change: fn(&mut Settings)| {
        let mut settings = Settings::default();
        change(&mut settings);
        let engine = parts(&home, home.path().join("secrets"))
            .engine(&settings, Probed::default())
            .await
            .unwrap();
        let support = *engine.support();
        (support.floor_backup, support.undo)
    };

    assert_eq!(support(|_| {}).await, (true, true), "the defaults keep the home store and undo");
    assert_eq!(support(|settings| settings.snapshot.enabled = false).await, (false, false));
    assert_eq!(support(|settings| settings.undo.enabled = false).await, (true, false));
    let always = |settings: &mut Settings| {
        settings.sandbox.ask_destructive = efr_config::AskDestructive::Always;
    };
    assert_eq!(support(always).await, (true, false), "always keeps every destructive exit");
}

#[tokio::test]
async fn engine_mount_view_follows_probed() {
    let (_root, home) = home();
    let parts = parts(&home, home.path().join("secrets"));
    let settings = Settings::default();
    let off = parts.engine(&settings, Probed::default()).await.unwrap();
    assert!(!off.support().mount_view, "no mount view before efrd checked its namespace");
    let probed = Probed { mount_view: true, ..Probed::default() };
    let on = parts.engine(&settings, probed).await.unwrap();
    assert!(on.support().mount_view);
    assert_eq!(on.locations(), off.locations(), "the probe changes only the support");
}

#[tokio::test]
async fn engine_netview_needs_setting_and_probe() {
    let (_root, home) = home();
    let parts = parts(&home, home.path().join("secrets"));
    let probed = Probed { netview: true, ..Probed::default() };
    let mut off = Settings::default();
    off.network.netview = false;
    let netview = async |settings: &Settings, probed: Probed| {
        parts.engine(settings, probed).await.unwrap().support().netview
    };
    assert!(netview(&Settings::default(), probed.clone()).await, "the key and the self-test");
    assert!(!netview(&Settings::default(), Probed::default()).await, "no passed self-test");
    assert!(!netview(&off, probed).await, "network.netview = false");
    assert!(!netview(&off, Probed::default()).await);
}

#[tokio::test]
async fn engine_bus_proxy_needs_setting_and_self_test() {
    let (_root, home) = home();
    let parts = parts(&home, home.path().join("secrets"));
    let passed = Probed { bus_proxy: true, ..Probed::default() };
    let mut on = Settings::default();
    on.dbus.proxy = true;
    let bus_proxy = async |settings: &Settings, probed: Probed| {
        parts.engine(settings, probed).await.unwrap().support().bus_proxy
    };
    assert!(bus_proxy(&on, passed.clone()).await, "the key and the self-test");
    assert!(!bus_proxy(&on, Probed::default()).await, "no passed self-test");
    let mut off = on.clone();
    off.dbus.proxy = false;
    assert!(!bus_proxy(&off, passed).await, "dbus.proxy = false");
}

#[tokio::test]
async fn network_mode_none_keeps_phase1() {
    let (_root, home) = home();
    let parts = parts(&home, home.path().join("secrets"));
    let probed = Probed { proxy: true, ..Probed::default() };
    let mut settings = Settings::default();
    settings.network.mode = efr_protocol::NetworkMode::None;
    let engine = parts.engine(&settings, probed).await.unwrap();
    assert_eq!(engine.support().egress, efr_permissions::Egress::None, "network.mode = none");
    let decided = decide(
        &engine,
        &home,
        Mode::Auto,
        Requirements::none()
            .with_command("curl https://extra.test/x")
            .with_command_dir(home.path()),
    );
    assert!(!matches!(decided, Effect::Allow), "a network need is still a host exit: {decided:?}");
}

#[tokio::test]
async fn failed_proxy_self_test_falls_back_to_none() {
    let (_root, home) = home();
    let parts = parts(&home, home.path().join("secrets"));
    let mut settings = Settings::default();
    settings.network.mode = efr_protocol::NetworkMode::Proxy;
    let failed = parts.engine(&settings, Probed::default()).await.unwrap();
    assert_eq!(failed.support().egress, efr_permissions::Egress::None, "no route-less proxy");
    let passed = Probed { proxy: true, ..Probed::default() };
    let on = parts.engine(&settings, passed).await.unwrap();
    assert_eq!(on.support().egress, efr_permissions::Egress::Proxy);
    assert_eq!(on.locations(), failed.locations(), "the probe changes only the support");
}

#[tokio::test]
async fn engine_hosts_follow_the_settings_and_the_mirrors() {
    let (_root, home) = home();
    let parts = parts(&home, home.path().join("secrets"));
    let mut settings = Settings::default();
    settings.network.allow = vec!["extra.test".to_owned(), "@pacman-mirrors".to_owned()];
    settings.network.deny = vec!["denied.test".to_owned()];
    let mirror =
        efr_permissions::MirrorHost { host: "mirror.test".to_owned(), port: 443, http: false };
    let probed = Probed { mirrors: std::sync::Arc::from(vec![mirror]), ..Probed::default() };
    let engine = parts.engine(&settings, probed).await.unwrap();
    assert!(engine.hosts().allows("extra.test", 443));
    assert!(engine.hosts().allows("mirror.test", 443), "@pacman-mirrors holds the read hosts");
    assert!(engine.hosts().denies("denied.test"));
    assert!(!engine.hosts().allows("other.test", 443));
}

#[tokio::test]
async fn df_routine_after_mount_view() {
    use std::sync::Arc;

    use efr_permissions::CallFacts;
    use efr_protocol::ExitKind;
    use efr_scope::Git;
    use efr_stdx::paths::Dirs;
    use efr_test_support::{TestClock, TestRng, TestStore};

    use crate::sandbox::{SandboxService, Seams, ServiceParts};

    let (root, home) = home();
    let dirs = Dirs::new(
        root.path().join("c"),
        root.path().join("d"),
        root.path().join("s"),
        root.path().join("r"),
    )
    .unwrap();
    let clock = TestClock::new().shared();
    let store = TestStore::open(Arc::clone(&clock)).await.unwrap();
    let service = SandboxService::new(ServiceParts {
        dirs,
        home: home.clone(),
        host: crate::sandbox::HostFacts::default(),
        source: None,
        seams: Seams::default(),
        registry: root.path().join("projects.toml"),
        writer: store.writer().clone(),
        clock: Arc::clone(&clock),
        rng: Arc::new(TestRng::new(5)),
        git: Git::new(clock).isolated(),
        programs: Default::default(),
    })
    .await;
    assert!(service.mount_view(), "the test runs in the mount namespace of its parent");
    let parts = parts(&home, home.path().join("secrets"));
    let settings = Settings::default();
    let engine = parts.engine(&settings, Probed::of(&service)).await.unwrap();
    assert!(engine.support().mount_view);

    let df = |engine: &Engine| {
        let facts = CallFacts {
            programs: vec![("df".to_owned(), Some(PathBuf::from("/usr/bin/df")), false)],
            ..CallFacts::default()
        };
        let requirements = Requirements::none()
            .with_command("df -hT")
            .with_command_dir(home.path())
            .with_facts(facts);
        let input = DecisionInput {
            requirements,
            scope: Scope::Machine,
            origin: Origin::Shell,
            mode: Mode::Auto,
            conversation_policy: ConversationPolicy::new(home.path().join("scratch")),
        };
        engine.decide(&input).exits().map(|need| need.kind).collect::<Vec<ExitKind>>()
    };
    assert_eq!(df(&engine), [], "df runs routine in the sandbox with the mount view");
    let before = parts.engine(&settings, Probed::default()).await.unwrap();
    assert_eq!(df(&before), [ExitKind::Outside], "without it, df runs outside");
}
