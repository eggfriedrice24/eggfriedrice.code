use std::path::{Path, PathBuf};

use efr_config::Settings;
use efr_permissions::{
    Action, CommandPattern, ConversationPolicy, DecisionInput, Effect, Engine, PathClass, Policy,
    Requirements, Resource, Rule,
};
use efr_protocol::{Mode, Origin, ProjectId, Scope};
use efr_scope::{Home, Registry};
use pretty_assertions::assert_eq;

use crate::engine::{EngineParts, build, load_registry, protected_config};

fn parts(home: &Home, secrets: PathBuf) -> EngineParts {
    EngineParts {
        home: home.clone(),
        secrets,
        registry: home.path().join(".config/efr/projects.toml"),
        config: home.path().join(".config/efr"),
        host: crate::sandbox::HostFacts::default(),
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

    let engine = parts(&home, home.path().join("secrets")).engine(&settings).await.unwrap();

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

    let engine = parts(&home, home.path().join("secrets")).engine(&settings).await.unwrap();

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

    let engine = parts(&home, secrets.clone()).engine(&settings).await.unwrap();

    let read = |path: PathBuf| Requirements::none().with_read(path);
    let key = read(home.path().join(".ssh/id_ed25519"));
    assert_eq!(decide(&engine, &home, Mode::Cautious, key), Effect::Allow);
    let token = read(secrets.join("openai-subscription.json"));
    assert_eq!(decide(&engine, &home, Mode::Cautious, token), Effect::Deny);
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

    let engine = parts(&home, home.path().join("secrets")).engine(&settings).await.unwrap();

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
    let engine = parts(&home, home.path().join("secrets")).engine(&settings).await.unwrap();
    let mut created = parts(&home, home.path().join("secrets"));
    created.config.clone_from(&other);
    let created = created.engine(&settings).await.unwrap();
    for mode in Mode::ALL {
        for path in [linked.path(), linked.target()] {
            let write = Requirements::none().with_write(path);
            assert_eq!(decide(&engine, &home, mode, write), Effect::Deny, "{mode} {path:?}");
        }
        let write = Requirements::none().with_write(missing.target());
        assert_eq!(decide(&created, &home, mode, write), Effect::Deny, "{mode}");
    }
}
