//! Decision tables: inputs as literals, one expected effect per row.

mod commands;
mod modes;
mod protection;

use std::path::PathBuf;

use efr_protocol::{Mode, Origin, ProjectId, Scope};
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
        .with_sealed_root("/home/u/.local/share/efr/secrets")
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
        mode: Mode::Cautious,
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
// Read-only commands run; other commands, network access and interactive calls need
// approval.
#[case::read_only_command(Requirements::none().with_command("ls -la"), Effect::Allow)]
#[case::command(Requirements::none().with_command("rm -rf build"), Effect::Ask)]
#[case::network(Requirements::none().with_network(), Effect::Ask)]
#[case::interactive(read(&scratch_file()).with_interactive(), Effect::Ask)]
// A call that declares nothing runs for the shell; see `no_requirements_by_origin`.
#[case::nothing(Requirements::none(), Effect::Allow)]
fn shell_turn_in_machine_scope(#[case] requirements: Requirements, #[case] expected: Effect) {
    assert_eq!(effect(requirements, Scope::Machine, Origin::Shell), expected);
}

#[rstest]
// User data inside the turn's registered project is free to write.
#[case::source(Scope::Project(app()), "/home/u/p/app/src/main.rs", Effect::Allow)]
#[case::dot_entry_in_project(Scope::Project(app()), "/home/u/p/app/.envrc", Effect::Allow)]
// Writing the root itself would replace or remove the whole project.
#[case::project_root(Scope::Project(app()), "/home/u/p/app", Effect::Ask)]
#[case::other_project_dir(Scope::Project(app()), "/home/u/p/other/a", Effect::Ask)]
#[case::sibling_prefix(Scope::Project(app()), "/home/u/p/application/a", Effect::Ask)]
#[case::config_from_project(Scope::Project(app()), "/home/u/.zshrc", Effect::Ask)]
#[case::scratch_from_project(
    Scope::Project(app()),
    "/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d/a",
    Effect::Allow
)]
#[case::escape_project(Scope::Project(app()), "/home/u/p/app/../../.ssh/config", Effect::Deny)]
// The repository's .git names programs that an allowed `git status` runs.
#[case::git_config(Scope::Project(app()), "/home/u/p/app/.git/config", Effect::Ask)]
#[case::git_hook(Scope::Project(app()), "/home/u/p/app/.git/hooks/pre-commit", Effect::Ask)]
#[case::git_file(Scope::Project(app()), "/home/u/p/app/sub/.git", Effect::Ask)]
#[case::git_in_scratch(
    Scope::Project(app()),
    "/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d/r/.git/config",
    Effect::Ask
)]
#[case::gitignore(Scope::Project(app()), "/home/u/p/app/.gitignore", Effect::Allow)]
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

/// The engine for a machine where `/home` links to `/var/home`: the daemon passes the
/// home directory as given and adds its resolved form, which is what tools declare.
fn linked_engine() -> Engine {
    Engine::with_defaults(locations().with_home_alias("/var/home/u").unwrap())
}

#[rstest]
// Secrets stay denied when a tool declares their resolved path.
#[case::read_ssh_key(read("/var/home/u/.ssh/id_rsa"), Scope::Machine, Effect::Deny)]
#[case::write_ssh_key(write("/var/home/u/.ssh/id_rsa"), Scope::Machine, Effect::Deny)]
#[case::write_ssh_from_project(
    write("/var/home/u/.ssh/authorized_keys"),
    Scope::Project(app()),
    Effect::Deny
)]
#[case::read_daemon_secrets(
    read("/var/home/u/.local/share/efr/secrets/openai-subscription.json"),
    Scope::Machine,
    Effect::Deny
)]
// The other classes decide as their lexical form does.
#[case::read_documents(read("/var/home/u/Documents/plan.md"), Scope::Machine, Effect::Allow)]
#[case::write_rc(write("/var/home/u/.zshrc"), Scope::Machine, Effect::Ask)]
#[case::write_scratch(
    write("/var/home/u/.local/share/efr/scratch/2026-10-04-fix-dns-0a1b2c3d/out.txt"),
    Scope::Machine,
    Effect::Allow
)]
#[case::write_project(write("/var/home/u/p/app/src/main.rs"), Scope::Project(app()), Effect::Allow)]
#[case::write_other_project(write("/var/home/u/p/other/a"), Scope::Project(app()), Effect::Ask)]
#[case::write_home_project(
    write("/var/home/u/notes.txt"),
    Scope::Project(home_project()),
    Effect::Ask
)]
#[case::read_lexical_ssh_key(read("/home/u/.ssh/id_rsa"), Scope::Machine, Effect::Deny)]
fn a_linked_home_decides_like_its_lexical_form(
    #[case] requirements: Requirements,
    #[case] scope: Scope,
    #[case] expected: Effect,
) {
    let decision = linked_engine().decide(&input(requirements, scope, Origin::Shell));
    assert_eq!(decision.effect(), expected);
}

#[test]
fn a_secret_behind_the_link_is_denied_by_the_secrets_rule_under_its_declared_path() {
    let input = input(read("/var/home/u/.ssh/id_rsa"), Scope::Machine, Origin::Shell);
    let decision = linked_engine().decide(&input);
    let deciding: Vec<_> = decision.deciding().map(ToString::to_string).collect();
    assert_eq!(
        deciding,
        ["read /var/home/u/.ssh/id_rsa (secrets): deny, by rule 7 of the machine policy"]
    );
}

#[test]
fn a_scratch_or_project_in_the_resolved_form_widens_both_forms() {
    let locations = Locations::new(HOME)
        .unwrap()
        .with_home_alias("/var/home/u")
        .unwrap()
        .with_project(app(), "/var/home/u/p/app")
        .unwrap();
    let engine = Engine::with_defaults(locations);
    let decide = |requirements: Requirements, scope: Scope| {
        let mut input = input(requirements, scope, Origin::Shell);
        input.conversation_policy =
            ConversationPolicy::new("/var/home/u/.local/share/efr/scratch/2026-10-04-a-00000000");
        engine.decide(&input).effect()
    };
    for form in [HOME, "/var/home/u"] {
        let scratch = format!("{form}/.local/share/efr/scratch/2026-10-04-a-00000000/out.txt");
        assert_eq!(decide(write(&scratch), Scope::Machine), Effect::Allow, "{form}");
        let source = format!("{form}/p/app/src/main.rs");
        assert_eq!(decide(write(&source), Scope::Project(app())), Effect::Allow, "{form}");
    }
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
#[case::nothing(Requirements::none(), Scope::Machine, Effect::Ask)]
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

#[rstest]
#[case::shell(Origin::Shell, Effect::Allow)]
#[case::cli(Origin::Cli, Effect::Allow)]
#[case::proxy(Origin::Proxy, Effect::Allow)]
#[case::phone(Origin::Phone, Effect::Ask)]
fn no_requirements_by_origin(#[case] origin: Origin, #[case] expected: Effect) {
    for scope in [Scope::Machine, Scope::Path("/srv".into()), Scope::Project(app())] {
        assert_eq!(effect(Requirements::none(), scope.clone(), origin), expected, "{scope:?}");
    }
}

#[test]
fn no_requirements_from_the_phone_ask_with_the_remote_reason() {
    let decision = engine().decide(&input(Requirements::none(), Scope::Machine, Origin::Phone));
    assert_eq!(
        decision.reasons(),
        [Reason {
            subject: Subject::Nothing,
            effect: Effect::Ask,
            cause: Cause::RemoteOrigin { origin: Origin::Phone },
        }]
    );
}

#[test]
fn no_requirements_from_a_local_origin_allow_with_their_own_reason() {
    let decision = engine().decide(&input(Requirements::none(), Scope::Machine, Origin::Cli));
    assert_eq!(
        decision.reasons(),
        [Reason { subject: Subject::Nothing, effect: Effect::Allow, cause: Cause::NoRequirements }]
    );
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
    // `cat` itself is allowed; the secret it would read is what denies the call.
    assert_eq!(effects, [Effect::Allow, Effect::Ask, Effect::Deny, Effect::Allow, Effect::Ask]);
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
    let engine = Engine::with_rules(locations(), configured);
    let decide =
        |path: &str, origin| engine.decide(&input(read(path), Scope::Machine, origin)).effect();
    assert_eq!(decide("/home/u/.ssh/config", Origin::Shell), Effect::Allow);
    assert_eq!(decide("/home/u/.ssh/config", Origin::Phone), Effect::Ask);
    assert_eq!(decide("/home/u/.ssh/id_ed25519", Origin::Shell), Effect::Deny);
}

/// The defaults followed by `rules`, as the daemon builds the machine policy from the
/// configuration.
fn configured(rules: Vec<Rule>) -> Engine {
    Engine::with_rules(locations(), Policy::new(rules).unwrap())
}

#[rstest]
#[case::read_any(Rule::new(Action::Read, Resource::Any, Effect::Allow))]
#[case::any_any(Rule::new(Action::Any, Resource::Any, Effect::Allow))]
#[case::under_home(Rule::new(Action::Read, Resource::Under("~".into()), Effect::Allow))]
#[case::under_root(Rule::new(Action::Any, Resource::Under("/".into()), Effect::Allow))]
#[case::under_proc(Rule::new(Action::Read, Resource::Under("/proc".into()), Effect::Allow))]
#[case::under_the_data_dir(Rule::new(
    Action::Read,
    Resource::Under("~/.local/share/efr".into()),
    Effect::Allow
))]
#[case::project(Rule::new(Action::Any, Resource::Project, Effect::Allow))]
#[case::broad_ask(Rule::new(Action::Read, Resource::Any, Effect::Ask))]
fn a_broad_user_rule_opens_no_secret(#[case] rule: Rule) {
    let engine = configured(vec![rule.clone()]);
    for path in
        ["/home/u/.ssh/id_ed25519", "/home/u/.aws/credentials", "/proc/self/environ", "/etc/shadow"]
    {
        let decision = engine.decide(&input(read(path), Scope::Project(app()), Origin::Shell));
        assert_eq!(decision.effect(), Effect::Deny, "{path} under {rule:?}");
        assert_eq!(decision.reasons()[0].cause, Cause::Rule { layer: Layer::Machine, index: 7 });
    }
}

const TOKEN: &str = "/home/u/.local/share/efr/secrets/openai-subscription.json";

#[rstest]
#[case::the_class(Rule::new(Action::Read, Resource::Class(PathClass::Secrets), Effect::Allow))]
#[case::the_secrets_dir(Rule::new(
    Action::Any,
    Resource::Under("~/.local/share/efr/secrets".into()),
    Effect::Allow
))]
#[case::the_token(Rule::new(Action::Read, Resource::Under(TOKEN.into()), Effect::Allow))]
#[case::everything(Rule::new(Action::Any, Resource::Any, Effect::Allow))]
fn no_user_rule_opens_the_daemons_own_secrets(#[case] rule: Rule) {
    let engine = configured(vec![rule.clone()]);
    for requirements in [read(TOKEN), write(TOKEN)] {
        let decision = engine.decide(&input(requirements, Scope::Machine, Origin::Shell));
        assert_eq!(decision.effect(), Effect::Deny, "{rule:?}");
        assert_eq!(decision.reasons()[0].cause, Cause::Sealed);
    }
}

#[test]
fn no_conversation_rule_opens_the_daemons_own_secrets() {
    let mut input = input(read(TOKEN), Scope::Machine, Origin::Shell);
    input.conversation_policy = ConversationPolicy::new(SCRATCH).with_rules(
        Policy::new(vec![Rule::new(Action::Any, Resource::Any, Effect::Allow)]).unwrap(),
    );
    assert_eq!(engine().decide(&input).effect(), Effect::Deny);
}

#[test]
fn a_rule_for_every_secret_opens_the_others_but_not_the_daemons() {
    let engine = configured(vec![Rule::new(
        Action::Read,
        Resource::Class(PathClass::Secrets),
        Effect::Allow,
    )]);
    let decide = |requirements| engine.decide(&input(requirements, Scope::Machine, Origin::Shell));
    assert_eq!(decide(read("/home/u/.ssh/id_ed25519")).effect(), Effect::Allow);
    let tree = decide(Requirements::none().with_read_tree("/home/u/.local/share/efr"));
    assert_eq!(tree.effect(), Effect::Ask);
    assert_eq!(
        tree.reasons()[0].cause,
        Cause::ReachesSecret { secret: "/home/u/.local/share/efr/secrets".into() }
    );
    let reason = decide(read(TOKEN)).reasons()[0].to_string();
    assert_eq!(
        reason,
        format!(
            "read {TOKEN} (secrets): deny, because efr keeps its own credentials there and no \
             rule opens them"
        )
    );
}

#[rstest]
#[case::the_class(Resource::Class(PathClass::Secrets), "/home/u/.ssh/id_ed25519", Effect::Allow)]
#[case::the_secret_dir(Resource::Under("~/.ssh".into()), "/home/u/.ssh/id_ed25519", Effect::Allow)]
#[case::below_a_secret(Resource::Under("~/.ssh/config".into()), "/home/u/.ssh/config", Effect::Allow)]
#[case::a_sibling_stays_closed(
    Resource::Under("~/.ssh/config".into()),
    "/home/u/.ssh/id_ed25519",
    Effect::Deny
)]
#[case::process_environment(
    Resource::Under("/proc/self/environ".into()),
    "/proc/self/environ",
    Effect::Allow
)]
fn a_user_rule_that_names_a_secret_opens_it(
    #[case] resource: Resource,
    #[case] path: &str,
    #[case] expected: Effect,
) {
    let engine = configured(vec![Rule::new(Action::Read, resource, Effect::Allow)]);
    let decision = engine.decide(&input(read(path), Scope::Machine, Origin::Shell));
    assert_eq!(decision.effect(), expected, "{path}");
}

#[rstest]
#[case::ask_for_everything(Effect::Ask)]
#[case::deny_everything(Effect::Deny)]
fn a_broad_user_rule_after_an_opened_secret_makes_it_stricter(#[case] broad: Effect) {
    let engine = configured(vec![
        Rule::new(Action::Read, Resource::Under("~/.ssh/config".into()), Effect::Allow),
        Rule::new(Action::Any, Resource::Any, broad),
    ]);
    let decision =
        engine.decide(&input(read("/home/u/.ssh/config"), Scope::Machine, Origin::Shell));
    let index = Policy::defaults().rules().len() + 1;
    assert_eq!(decision.effect(), broad);
    assert_eq!(decision.reasons()[0].cause, Cause::Rule { layer: Layer::Machine, index });
}

#[test]
fn a_broad_conversation_rule_makes_an_opened_secret_stricter() {
    let engine = configured(vec![Rule::new(
        Action::Read,
        Resource::Under("~/.ssh/config".into()),
        Effect::Allow,
    )]);
    let mut input = input(read("/home/u/.ssh/config"), Scope::Machine, Origin::Shell);
    input.conversation_policy = ConversationPolicy::new(SCRATCH)
        .with_rules(Policy::new(vec![Rule::new(Action::Any, Resource::Any, Effect::Ask)]).unwrap());
    let decision = engine.decide(&input);
    assert_eq!(decision.effect(), Effect::Ask);
    assert_eq!(decision.reasons()[0].cause, Cause::Rule { layer: Layer::Conversation, index: 0 });
}

#[test]
fn an_empty_machine_policy_denies_what_no_rule_names() {
    let engine = Engine::with_policy(locations(), Policy::empty());
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
    assert_eq!(engine.policy(Mode::Cautious), &Policy::defaults());
    assert_eq!(engine.policy(Mode::Manual), &Policy::base(Mode::Manual));
    assert_eq!(engine.policy(Mode::Auto), &Policy::base(Mode::Auto));
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
