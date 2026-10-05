use std::path::PathBuf;

use efr_config::Settings;
use efr_permissions::{
    Action, CommandPattern, ConversationPolicy, DecisionInput, Effect, Engine, PathClass, Policy,
    Requirements, Resource, Rule,
};
use efr_protocol::{Mode, Origin, ProjectId, Scope};
use efr_scope::{Home, Registry};
use pretty_assertions::assert_eq;

use crate::engine::{EngineParts, build, load_registry};

fn parts(home: &Home, secrets: PathBuf) -> EngineParts {
    EngineParts {
        home: home.clone(),
        secrets,
        registry: home.path().join(".config/efr/projects.toml"),
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
    let engine =
        build(&home, &home.path().join("secrets"), &Settings::default(), &projects).unwrap();

    let input = DecisionInput {
        requirements: Requirements::none().with_write(app.join("src/main.rs")),
        scope: Scope::Project(id),
        origin: Origin::Shell,
        mode: Mode::Cautious,
        conversation_policy: ConversationPolicy::new(home.path().join("scratch")),
    };
    assert_eq!(engine.decide(&input).effect(), Effect::Allow);
}
