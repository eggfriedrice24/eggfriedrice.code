use std::path::PathBuf;

use efr_config::Settings;
use efr_permissions::{
    Action, CommandPattern, ConversationPolicy, DecisionInput, Effect, PathClass, Policy,
    Requirements, Resource, Rule,
};
use efr_protocol::{Origin, ProjectId, Scope};
use efr_scope::{Home, Registry};
use pretty_assertions::assert_eq;

use crate::engine::{EngineParts, build, load_registry};

fn parts(home: &Home, secrets: PathBuf) -> EngineParts {
    EngineParts { home: home.clone(), secrets, registry: home.path().join("projects.toml") }
}

#[tokio::test]
async fn the_secret_paths_of_the_config_classify_as_secrets() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    let home = Home::new(&home).unwrap();
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
async fn the_engine_decides_by_the_built_in_rules_then_the_users() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    let home = Home::new(&home).unwrap();
    let rule = Rule::new(
        Action::Execute,
        Resource::Command(CommandPattern::new("cargo").with_args(["test"])),
        Effect::Allow,
    );
    let mut settings = Settings::default();
    settings.permissions.rules = Policy::new(vec![rule.clone()]).unwrap();

    let engine = parts(&home, home.path().join("secrets")).engine(&settings).await.unwrap();

    let defaults = Policy::defaults();
    let rules = engine.policy().rules();
    assert_eq!(&rules[..defaults.rules().len()], defaults.rules());
    assert_eq!(rules[defaults.rules().len()..], [rule]);
}

#[tokio::test]
async fn no_rule_of_the_users_opens_the_daemons_own_secrets() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    let home = Home::new(&home).unwrap();
    let every_secret = Rule::new(Action::Read, Resource::Class(PathClass::Secrets), Effect::Allow);
    let mut settings = Settings::default();
    settings.permissions.rules = Policy::new(vec![every_secret]).unwrap();
    let secrets = home.path().join(".local/share/efr/secrets");

    let engine = parts(&home, secrets.clone()).engine(&settings).await.unwrap();

    let decide = |path: PathBuf| {
        let input = DecisionInput {
            requirements: Requirements::none().with_read(path),
            scope: Scope::Machine,
            origin: Origin::Shell,
            conversation_policy: ConversationPolicy::new(home.path().join("scratch")),
        };
        engine.decide(&input).effect()
    };
    assert_eq!(decide(home.path().join(".ssh/id_ed25519")), Effect::Allow);
    assert_eq!(decide(secrets.join("openai-subscription.json")), Effect::Deny);
}

#[tokio::test]
async fn a_registered_project_is_writable_and_a_missing_registry_registers_none() {
    let root = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(root.path()).unwrap();
    let home = Home::new(&home).unwrap();
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
        conversation_policy: ConversationPolicy::new(home.path().join("scratch")),
    };
    assert_eq!(engine.decide(&input).effect(), Effect::Allow);
}
