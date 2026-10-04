//! Decision tables: inputs as literals, one expected effect per row.

use std::path::PathBuf;

use efr_protocol::{Origin, ProjectId, Scope};
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use rstest::rstest;

use super::Engine;
use crate::{
    Action, Cause, CommandPattern, ConversationPolicy, DecisionInput, Effect, Layer, Locations,
    PathClass, Policy, Reason, Requirements, Resource, Rule, Subject,
};

const HOME: &str = "/home/u";
const SCRATCH: &str = "/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d";

fn id(n: u8) -> ProjectId {
    format!("0192f0c1-7a00-7000-8000-0000000000{n:02}").parse().unwrap()
}

/// `~/p/app`: an ordinary registered project.
fn app() -> ProjectId {
    id(1)
}

/// `~` registered as a project.
fn home_project() -> ProjectId {
    id(2)
}

/// `/` registered as a project.
fn root_project() -> ProjectId {
    id(3)
}

/// `/home`, above `~`, registered as a project.
fn above_home_project() -> ProjectId {
    id(4)
}

/// `/srv/site`, outside `~`, registered as a project.
fn site() -> ProjectId {
    id(5)
}

/// A project id that the engine does not know.
fn unknown_project() -> ProjectId {
    id(6)
}

fn locations() -> Locations {
    Locations::new(HOME)
        .unwrap()
        .with_secret_root("/home/u/.local/share/efr/secrets")
        .unwrap()
        .with_project(app(), "/home/u/p/app")
        .unwrap()
        .with_project(home_project(), HOME)
        .unwrap()
        .with_project(root_project(), "/")
        .unwrap()
        .with_project(above_home_project(), "/home")
        .unwrap()
        .with_project(site(), "/srv/site")
        .unwrap()
}

fn engine() -> Engine {
    Engine::with_defaults(locations())
}

fn input(requirements: Requirements, scope: Scope, origin: Origin) -> DecisionInput {
    DecisionInput {
        requirements,
        scope,
        origin,
        conversation_policy: ConversationPolicy::new(SCRATCH),
    }
}

fn effect(requirements: Requirements, scope: Scope, origin: Origin) -> Effect {
    engine().decide(&input(requirements, scope, origin)).effect()
}

fn read(path: &str) -> Requirements {
    Requirements::none().with_read(path)
}

fn write(path: &str) -> Requirements {
    Requirements::none().with_write(path)
}

fn scratch_file() -> String {
    format!("{SCRATCH}/out.txt")
}

#[rstest]
// Scratch is free.
#[case::read_scratch(read(&scratch_file()), Effect::Allow)]
#[case::write_scratch(write(&scratch_file()), Effect::Allow)]
#[case::write_scratch_dir(write(SCRATCH), Effect::Allow)]
// Reading is free outside secrets.
#[case::read_rc(read("/home/u/.zshrc"), Effect::Allow)]
#[case::read_documents(read("/home/u/Documents/plan.md"), Effect::Allow)]
#[case::read_etc(read("/etc/hosts"), Effect::Allow)]
#[case::read_home(read(HOME), Effect::Allow)]
#[case::read_root(read("/"), Effect::Allow)]
// Writing outside scratch needs approval.
#[case::write_rc(write("/home/u/.zshrc"), Effect::Ask)]
#[case::write_config(write("/home/u/.config/nvim/init.lua"), Effect::Ask)]
#[case::write_documents(write("/home/u/Documents/plan.md"), Effect::Ask)]
#[case::write_project_without_scope(write("/home/u/p/app/src/main.rs"), Effect::Ask)]
#[case::write_cache(write("/home/u/.cache/x"), Effect::Ask)]
#[case::write_local_share(write("/home/u/.local/share/x"), Effect::Ask)]
#[case::write_other_scratch(
    write("/home/u/.local/share/efr/scratch/2026-10-03-a-99999999/x"),
    Effect::Ask
)]
#[case::write_etc(write("/etc/hosts"), Effect::Ask)]
#[case::write_unit(write("/etc/systemd/system/x.service"), Effect::Ask)]
#[case::write_usr(write("/usr/local/bin/x"), Effect::Ask)]
// Secrets are denied by default, for reading and writing.
#[case::read_ssh_key(read("/home/u/.ssh/id_ed25519"), Effect::Deny)]
#[case::write_ssh_config(write("/home/u/.ssh/config"), Effect::Deny)]
#[case::list_ssh_dir(read("/home/u/.ssh"), Effect::Deny)]
#[case::read_gnupg(read("/home/u/.gnupg/private-keys-v1.d/x.key"), Effect::Deny)]
#[case::read_pass(read("/home/u/.password-store/mail.gpg"), Effect::Deny)]
#[case::read_keyring(read("/home/u/.local/share/keyrings/login.keyring"), Effect::Deny)]
#[case::read_daemon_secrets(
    read("/home/u/.local/share/efr/secrets/openai-subscription.json"),
    Effect::Deny
)]
#[case::read_shadow(read("/etc/shadow"), Effect::Deny)]
#[case::escape_from_scratch(read(&format!("{SCRATCH}/../../secrets/openai-subscription.json")), Effect::Deny)]
#[case::escape_by_dots(read("/home/u/p/app/../../.ssh/id_ed25519"), Effect::Deny)]
// A path whose class is unknown is denied.
#[case::relative_read(read("notes.txt"), Effect::Deny)]
#[case::relative_write(write("../.zshrc"), Effect::Deny)]
// Commands, network access and interactive calls need approval.
#[case::command(Requirements::none().with_command("ls -la"), Effect::Ask)]
#[case::network(Requirements::none().with_network(), Effect::Ask)]
#[case::interactive(read(&scratch_file()).with_interactive(), Effect::Ask)]
// A call that declares nothing runs.
#[case::nothing(Requirements::none(), Effect::Allow)]
fn shell_turn_in_machine_scope(#[case] requirements: Requirements, #[case] expected: Effect) {
    assert_eq!(effect(requirements, Scope::Machine, Origin::Shell), expected);
}

#[rstest]
// User data inside the turn's registered project is free to write.
#[case::source(Scope::Project(app()), "/home/u/p/app/src/main.rs", Effect::Allow)]
#[case::dot_entry_in_project(Scope::Project(app()), "/home/u/p/app/.envrc", Effect::Allow)]
#[case::project_root(Scope::Project(app()), "/home/u/p/app", Effect::Allow)]
#[case::other_project_dir(Scope::Project(app()), "/home/u/p/other/a", Effect::Ask)]
#[case::sibling_prefix(Scope::Project(app()), "/home/u/p/application/a", Effect::Ask)]
#[case::config_from_project(Scope::Project(app()), "/home/u/.zshrc", Effect::Ask)]
#[case::scratch_from_project(
    Scope::Project(app()),
    "/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d/a",
    Effect::Allow
)]
#[case::escape_project(Scope::Project(app()), "/home/u/p/app/../../.ssh/config", Effect::Deny)]
// Home and root are never treated as a project, even when registered.
#[case::home_project_file(Scope::Project(home_project()), "/home/u/notes.txt", Effect::Ask)]
#[case::home_project_inner(
    Scope::Project(home_project()),
    "/home/u/p/app/src/main.rs",
    Effect::Ask
)]
#[case::home_project_config(Scope::Project(home_project()), "/home/u/.zshrc", Effect::Ask)]
#[case::root_project_home(Scope::Project(root_project()), "/home/u/notes.txt", Effect::Ask)]
#[case::root_project_srv(Scope::Project(root_project()), "/srv/site/index.html", Effect::Ask)]
#[case::above_home_project(Scope::Project(above_home_project()), "/home/u/notes.txt", Effect::Ask)]
// Only user data widens: a project outside home holds system paths.
#[case::system_project(Scope::Project(site()), "/srv/site/index.html", Effect::Ask)]
// Only a registered, known project widens.
#[case::unknown_project(Scope::Project(unknown_project()), "/home/u/p/app/a", Effect::Ask)]
#[case::path_scope(Scope::Path("/home/u/p/app".into()), "/home/u/p/app/a", Effect::Ask)]
#[case::machine_scope(Scope::Machine, "/home/u/p/app/a", Effect::Ask)]
fn writes_by_scope(#[case] scope: Scope, #[case] path: &str, #[case] expected: Effect) {
    assert_eq!(effect(write(path), scope, Origin::Shell), expected);
}

#[rstest]
#[case::read_scratch(read(&scratch_file()), Scope::Machine, Effect::Allow)]
#[case::write_scratch(write(&scratch_file()), Scope::Machine, Effect::Allow)]
#[case::read_rc(read("/home/u/.zshrc"), Scope::Machine, Effect::Ask)]
#[case::read_documents(read("/home/u/Documents/plan.md"), Scope::Machine, Effect::Ask)]
#[case::read_etc(read("/etc/hosts"), Scope::Machine, Effect::Ask)]
#[case::write_project(write("/home/u/p/app/src/main.rs"), Scope::Project(app()), Effect::Ask)]
#[case::write_etc(write("/etc/hosts"), Scope::Machine, Effect::Ask)]
#[case::read_secret(read("/home/u/.ssh/id_ed25519"), Scope::Machine, Effect::Deny)]
#[case::relative(read("a"), Scope::Machine, Effect::Deny)]
#[case::command(Requirements::none().with_command("ls"), Scope::Machine, Effect::Ask)]
#[case::network(Requirements::none().with_network(), Scope::Machine, Effect::Ask)]
#[case::nothing(Requirements::none(), Scope::Machine, Effect::Allow)]
fn phone_tightens_every_class_but_scratch_to_ask(
    #[case] requirements: Requirements,
    #[case] scope: Scope,
    #[case] expected: Effect,
) {
    assert_eq!(effect(requirements, scope, Origin::Phone), expected);
}

#[rstest]
#[case::cli(Origin::Cli)]
#[case::proxy(Origin::Proxy)]
fn local_origins_decide_like_the_shell(#[case] origin: Origin) {
    for requirements in [
        read("/home/u/.zshrc"),
        write("/home/u/p/app/a"),
        read("/home/u/.ssh/id_ed25519"),
        Requirements::none().with_command("ls"),
    ] {
        let scope = Scope::Project(app());
        assert_eq!(
            effect(requirements.clone(), scope.clone(), origin),
            effect(requirements, scope, Origin::Shell)
        );
    }
}

#[test]
fn the_phone_reason_replaces_the_allowing_rule() {
    let decision = engine().decide(&input(read("/etc/hosts"), Scope::Machine, Origin::Phone));
    assert_eq!(
        decision.reasons(),
        [Reason {
            subject: Subject::Path {
                path: "/etc/hosts".into(),
                access: crate::Access::Read,
                class: Some(PathClass::System),
            },
            effect: Effect::Ask,
            cause: Cause::RemoteOrigin { origin: Origin::Phone },
        }]
    );
}

#[test]
fn every_requirement_gets_a_reason_and_the_strictest_decides() {
    let requirements = Requirements::none()
        .with_read(scratch_file())
        .with_write("/home/u/.zshrc")
        .with_read("/home/u/.ssh/id_ed25519")
        .with_command("cat ~/.ssh/id_ed25519")
        .with_interactive();
    let decision = engine().decide(&input(requirements, Scope::Machine, Origin::Shell));
    let effects: Vec<_> = decision.reasons().iter().map(|reason| reason.effect).collect();
    assert_eq!(effects, [Effect::Allow, Effect::Ask, Effect::Deny, Effect::Ask, Effect::Ask]);
    assert_eq!(decision.effect(), Effect::Deny);
    let deciding: Vec<_> = decision.deciding().map(ToString::to_string).collect();
    assert_eq!(
        deciding,
        ["read /home/u/.ssh/id_ed25519 (secrets): deny, by rule 7 of the machine policy"]
    );
}

#[test]
fn reasons_name_the_normalised_path_and_its_class() {
    let decision = engine().decide(&input(
        write("/home/u/p/app/../../.config/./nvim/init.lua"),
        Scope::Machine,
        Origin::Shell,
    ));
    assert_eq!(
        decision.reasons()[0].to_string(),
        "write /home/u/.config/nvim/init.lua (user config): ask, by rule 4 of the machine policy"
    );
}

#[rstest]
#[case::home(HOME)]
#[case::root("/")]
#[case::above_home("/home")]
#[case::relative("scratch/x")]
fn a_scratch_that_covers_home_frees_nothing(#[case] scratch: &str) {
    let mut input = input(write("/home/u/notes.txt"), Scope::Machine, Origin::Shell);
    input.conversation_policy = ConversationPolicy::new(scratch);
    assert_eq!(engine().decide(&input).effect(), Effect::Ask);
    input.requirements = write("/etc/hosts");
    assert_eq!(engine().decide(&input).effect(), Effect::Ask);
}

fn with_conversation_rules(
    requirements: Requirements,
    origin: Origin,
    scope: Scope,
    rules: Vec<Rule>,
) -> Effect {
    let mut input = input(requirements, scope, origin);
    input.conversation_policy =
        ConversationPolicy::new(SCRATCH).with_rules(Policy::new(rules).unwrap());
    engine().decide(&input).effect()
}

#[rstest]
// A conversation may loosen user config, user data, commands and network access.
#[case::allow_config_pattern(
    write("/home/u/.config/nvim/init.lua"), Origin::Shell, Scope::Machine,
    Rule::new(Action::Write, Resource::Under("~/.config/nvim".into()), Effect::Allow),
    Effect::Allow
)]
#[case::pattern_does_not_spread(
    write("/home/u/.config/zsh/.zshrc"), Origin::Shell, Scope::Machine,
    Rule::new(Action::Write, Resource::Under("~/.config/nvim".into()), Effect::Allow),
    Effect::Ask
)]
#[case::allow_documents(
    write("/home/u/Documents/plan.md"),
    Origin::Shell,
    Scope::Machine,
    Rule::new(Action::Write, Resource::Class(PathClass::UserData), Effect::Allow),
    Effect::Allow
)]
#[case::allow_command(
    Requirements::none().with_command("git status --short"), Origin::Shell, Scope::Machine,
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("git").with_args(["status"])), Effect::Allow),
    Effect::Allow
)]
#[case::allowed_command_cannot_chain(
    Requirements::none().with_command("git status; rm -rf ~"), Origin::Shell, Scope::Machine,
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("git").with_args(["status"])), Effect::Allow),
    Effect::Ask
)]
#[case::allow_network(
    Requirements::none().with_network(), Origin::Shell, Scope::Machine,
    Rule::new(Action::Network, Resource::Any, Effect::Allow),
    Effect::Allow
)]
// The phone still needs approval after a conversation rule allows something.
#[case::phone_config_pattern(
    write("/home/u/.config/nvim/init.lua"), Origin::Phone, Scope::Machine,
    Rule::new(Action::Write, Resource::Under("~/.config/nvim".into()), Effect::Allow),
    Effect::Ask
)]
#[case::phone_command(
    Requirements::none().with_command("git status"), Origin::Phone, Scope::Machine,
    Rule::new(Action::Execute, Resource::Command(CommandPattern::new("git")), Effect::Allow),
    Effect::Ask
)]
// A conversation never loosens secrets or system paths.
#[case::secret_stays_denied(
    read("/home/u/.ssh/config"), Origin::Shell, Scope::Machine,
    Rule::new(Action::Any, Resource::Under("~/.ssh".into()), Effect::Allow),
    Effect::Deny
)]
#[case::secret_stays_denied_by_class(
    read("/home/u/.gnupg/pubring.kbx"),
    Origin::Shell,
    Scope::Machine,
    Rule::new(Action::Any, Resource::Class(PathClass::Secrets), Effect::Ask),
    Effect::Deny
)]
#[case::system_write_stays_ask(
    write("/etc/hosts"),
    Origin::Shell,
    Scope::Machine,
    Rule::new(Action::Write, Resource::Class(PathClass::System), Effect::Allow),
    Effect::Ask
)]
#[case::catch_all_does_not_open_system(
    write("/etc/nixos/configuration.nix"),
    Origin::Shell,
    Scope::Machine,
    Rule::new(Action::Any, Resource::Any, Effect::Allow),
    Effect::Ask
)]
// A conversation may tighten anything.
#[case::deny_project_dir(
    write("/home/u/p/app/migrations/1.sql"), Origin::Shell, Scope::Project(app()),
    Rule::new(Action::Write, Resource::Under("~/p/app/migrations".into()), Effect::Deny),
    Effect::Deny
)]
#[case::ask_in_project(
    write("/home/u/p/app/src/main.rs"),
    Origin::Shell,
    Scope::Project(app()),
    Rule::new(Action::Write, Resource::Project, Effect::Ask),
    Effect::Ask
)]
#[case::deny_system_reads(
    read("/etc/hosts"),
    Origin::Shell,
    Scope::Machine,
    Rule::new(Action::Read, Resource::Class(PathClass::System), Effect::Deny),
    Effect::Deny
)]
#[case::ask_system_reads(
    read("/etc/hosts"),
    Origin::Shell,
    Scope::Machine,
    Rule::new(Action::Read, Resource::Class(PathClass::System), Effect::Ask),
    Effect::Ask
)]
#[case::deny_scratch(
    write(&scratch_file()), Origin::Shell, Scope::Machine,
    Rule::new(Action::Write, Resource::Class(PathClass::Scratch), Effect::Deny),
    Effect::Deny
)]
fn conversation_rules(
    #[case] requirements: Requirements,
    #[case] origin: Origin,
    #[case] scope: Scope,
    #[case] rule: Rule,
    #[case] expected: Effect,
) {
    assert_eq!(with_conversation_rules(requirements, origin, scope, vec![rule]), expected);
}

#[test]
fn a_conversation_rule_that_decides_is_named_in_the_reason() {
    let mut input = input(write("/home/u/.zshrc"), Scope::Machine, Origin::Shell);
    let rules = vec![
        Rule::new(Action::Read, Resource::Any, Effect::Allow),
        Rule::new(Action::Write, Resource::Under("~/.zshrc".into()), Effect::Allow),
    ];
    input.conversation_policy =
        ConversationPolicy::new(SCRATCH).with_rules(Policy::new(rules).unwrap());
    let decision = engine().decide(&input);
    assert_eq!(decision.reasons()[0].cause, Cause::Rule { layer: Layer::Conversation, index: 1 });
}

#[test]
fn the_machine_policy_may_open_a_secret_explicitly() {
    let configured = Policy::new(vec![Rule::new(
        Action::Read,
        Resource::Under("~/.ssh/config".into()),
        Effect::Allow,
    )])
    .unwrap();
    let engine = Engine::new(locations(), Policy::defaults().then(configured));
    let decide =
        |path: &str, origin| engine.decide(&input(read(path), Scope::Machine, origin)).effect();
    assert_eq!(decide("/home/u/.ssh/config", Origin::Shell), Effect::Allow);
    assert_eq!(decide("/home/u/.ssh/config", Origin::Phone), Effect::Ask);
    assert_eq!(decide("/home/u/.ssh/id_ed25519", Origin::Shell), Effect::Deny);
}

#[test]
fn an_empty_machine_policy_denies_what_no_rule_names() {
    let engine = Engine::new(locations(), Policy::empty());
    let decision = engine.decide(&input(read(&scratch_file()), Scope::Machine, Origin::Shell));
    assert_eq!(decision.effect(), Effect::Deny);
    assert_eq!(decision.reasons()[0].cause, Cause::NoRule);
    // A conversation rule decides when the machine has no rule for a free class.
    let mut input = input(read(&scratch_file()), Scope::Machine, Origin::Shell);
    input.conversation_policy = ConversationPolicy::new(SCRATCH).with_rules(
        Policy::new(vec![Rule::new(Action::Read, Resource::Any, Effect::Allow)]).unwrap(),
    );
    assert_eq!(engine.decide(&input).effect(), Effect::Allow);
    // But not for a system path, where the missing rule still denies.
    input.requirements = read("/etc/hosts");
    assert_eq!(engine.decide(&input).effect(), Effect::Deny);
}

#[test]
fn the_engine_exposes_what_it_was_built_from() {
    let engine = engine();
    assert_eq!(engine.locations(), &locations());
    assert_eq!(engine.policy(), &Policy::defaults());
}

fn any_path() -> impl Strategy<Value = PathBuf> {
    let prefix = prop::sample::select(vec![
        "/home/u",
        "/home/u/.config",
        "/home/u/.ssh",
        "/home/u/p/app",
        "/home/u/Documents",
        "/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d",
        "/home/u/.local/share/efr/secrets",
        "/etc",
        "/srv/site",
        "/",
        "relative",
    ]);
    let tail =
        prop::collection::vec(prop::sample::select(vec!["a", "b", ".zshrc", "..", "."]), 0..4);
    (prefix, tail).prop_map(|(prefix, tail)| {
        tail.into_iter().fold(PathBuf::from(prefix), |path, segment| path.join(segment))
    })
}

fn any_requirements() -> impl Strategy<Value = Requirements> {
    let paths = prop::collection::vec((any_path(), any::<bool>()), 0..4);
    let command = prop::option::of(prop::sample::select(vec!["ls", "git status", "rm -rf ~"]));
    (paths, command, any::<bool>(), any::<bool>()).prop_map(
        |(paths, command, network, interactive)| {
            let mut requirements = Requirements { network, interactive, ..Requirements::none() };
            requirements.command = command.map(str::to_owned);
            for (path, is_write) in paths {
                requirements = if is_write {
                    requirements.with_write(path)
                } else {
                    requirements.with_read(path)
                };
            }
            requirements
        },
    )
}

fn any_scope() -> impl Strategy<Value = Scope> {
    prop::sample::select(vec![
        Scope::Machine,
        Scope::Path("/home/u/p/app".into()),
        Scope::Project(app()),
        Scope::Project(home_project()),
        Scope::Project(root_project()),
        Scope::Project(site()),
        Scope::Project(unknown_project()),
    ])
}

fn any_conversation_rules() -> impl Strategy<Value = Policy> {
    let rule = prop::sample::select(vec![
        Rule::new(Action::Any, Resource::Any, Effect::Allow),
        Rule::new(Action::Write, Resource::Under("~".into()), Effect::Allow),
        Rule::new(Action::Any, Resource::Under("~/.ssh".into()), Effect::Allow),
        Rule::new(Action::Any, Resource::Class(PathClass::Secrets), Effect::Allow),
        Rule::new(Action::Write, Resource::Class(PathClass::System), Effect::Allow),
        Rule::new(Action::Execute, Resource::Command(CommandPattern::new("ls")), Effect::Allow),
        Rule::new(Action::Write, Resource::Project, Effect::Deny),
    ]);
    prop::collection::vec(rule, 0..4).prop_map(|rules| Policy::new(rules).unwrap())
}

proptest! {
    #[test]
    fn the_phone_is_never_looser_than_the_shell(
        requirements in any_requirements(),
        scope in any_scope(),
        rules in any_conversation_rules(),
    ) {
        let engine = engine();
        let decide = |origin| {
            let mut input = input(requirements.clone(), scope.clone(), origin);
            input.conversation_policy = ConversationPolicy::new(SCRATCH).with_rules(rules.clone());
            engine.decide(&input).effect()
        };
        prop_assert!(decide(Origin::Phone) >= decide(Origin::Shell));
    }

    #[test]
    fn another_requirement_never_loosens_the_decision(
        requirements in any_requirements(),
        extra in any_requirements(),
        scope in any_scope(),
    ) {
        let engine = engine();
        let before = engine.decide(&input(requirements.clone(), scope.clone(), Origin::Shell));
        let mut more = requirements;
        more.paths.extend(extra.paths);
        more.command = more.command.or(extra.command);
        more.network |= extra.network;
        more.interactive |= extra.interactive;
        let after = engine.decide(&input(more, scope, Origin::Shell));
        prop_assert!(after.effect() >= before.effect());
    }

    #[test]
    fn secrets_stay_denied_whatever_the_conversation_says(
        tail in prop::sample::select(vec!["id_ed25519", "config", "known_hosts", "."]),
        is_write in any::<bool>(),
        scope in any_scope(),
        rules in any_conversation_rules(),
        phone in any::<bool>(),
    ) {
        let path = format!("/home/u/.ssh/{tail}");
        let requirements =
            if is_write { Requirements::none().with_write(path) } else { Requirements::none().with_read(path) };
        let origin = if phone { Origin::Phone } else { Origin::Shell };
        let mut input = input(requirements, scope, origin);
        input.conversation_policy = ConversationPolicy::new(SCRATCH).with_rules(rules);
        prop_assert_eq!(engine().decide(&input).effect(), Effect::Deny);
    }
}
