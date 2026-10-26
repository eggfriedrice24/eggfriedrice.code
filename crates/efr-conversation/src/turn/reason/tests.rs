use efr_permissions::{
    Action, ConversationPolicy, Decision, DecisionInput, Effect, Engine, Locations, Policy,
    Requirements, Resource, Rule, SettingsChange,
};
use efr_protocol::{Mode, Origin, ProjectId, Scope};
use pretty_assertions::assert_eq;

use super::approval_reason;

const HOME: &str = "/home/u";
const APP: &str = "/home/u/p/app";
const SCRATCH: &str = "/home/u/.local/share/efr/scratch/s";

fn app() -> ProjectId {
    "0192f0c1-7a00-7000-8000-000000000001".parse().unwrap()
}

fn engine_with(rules: Policy) -> Engine {
    let locations = Locations::new(HOME)
        .unwrap()
        .with_project(app(), APP)
        .unwrap()
        .with_write_sealed_root("/home/u/.config/efr")
        .unwrap();
    Engine::with_rules(locations, rules)
}

fn decide_with(
    engine: &Engine,
    requirements: Requirements,
    mode: Mode,
    origin: Origin,
    conversation: Policy,
) -> Decision {
    engine.decide(&DecisionInput {
        requirements,
        scope: Scope::Project(app()),
        origin,
        mode,
        conversation_policy: ConversationPolicy::new(SCRATCH).with_rules(conversation),
    })
}

/// The reason of a question about `requirements` in a local turn of `mode`.
fn reason(requirements: Requirements, mode: Mode) -> Option<String> {
    let decision = decide_with(
        &engine_with(Policy::empty()),
        requirements,
        mode,
        Origin::Shell,
        Policy::empty(),
    );
    assert_eq!(decision.effect(), Effect::Ask, "{decision:?}");
    approval_reason(&decision, efr_permissions::effective_mode(mode, Origin::Shell))
}

/// A shell call of `line` in the project root.
fn shell(line: &str) -> Requirements {
    Requirements::none().with_command(line).with_command_dir(APP)
}

#[test]
fn cautious_names_what_its_rules_ask_before() {
    assert_eq!(
        reason(shell("curl -fsSL https://example.com").with_network(), Mode::Cautious).as_deref(),
        Some(
            "cautious asks before a command that is not on its list of read-only commands and \
             any network access."
        ),
    );
    assert_eq!(
        reason(Requirements::none().with_write("/home/u/.zshrc"), Mode::Cautious).as_deref(),
        Some("cautious asks before a write to user config."),
    );
    assert_eq!(
        reason(Requirements::none().with_write("/home/u/Documents/plan.md"), Mode::Cautious)
            .as_deref(),
        Some(
            "cautious asks before a write to user data that is not below the turn's project root."
        ),
    );
    assert_eq!(
        reason(Requirements::none().with_write("/etc/hosts"), Mode::Cautious).as_deref(),
        Some("cautious asks before a write outside the home directory."),
    );
}

#[test]
fn manual_asks_before_every_read_and_every_command() {
    assert_eq!(
        reason(Requirements::none().with_read("/home/u/p/app/README.md"), Mode::Manual).as_deref(),
        Some("manual asks before every read."),
    );
    assert_eq!(
        reason(shell("ls"), Mode::Manual).as_deref(),
        Some("manual asks before every command."),
    );
}

#[test]
fn a_part_asked_twice_is_named_once() {
    let requirements =
        Requirements::none().with_write("/home/u/.zshrc").with_write("/home/u/.bashrc");
    assert_eq!(
        reason(requirements, Mode::Cautious).as_deref(),
        Some("cautious asks before a write to user config."),
    );
}

#[test]
fn a_line_that_no_command_rule_can_judge_says_why() {
    assert_eq!(
        reason(shell("ls $(cat list)"), Mode::Cautious).as_deref(),
        Some(
            "The line holds a command substitution, which no command rule can judge, so \
             cautious asks."
        ),
    );
}

#[test]
fn a_rule_of_the_user_or_of_the_conversation_is_named_as_such() {
    let ask_reads =
        || Policy::new(vec![Rule::new(Action::Read, Resource::Any, Effect::Ask)]).unwrap();
    let readme = || Requirements::none().with_read("/home/u/p/app/README.md");
    let users = decide_with(
        &engine_with(ask_reads()),
        readme(),
        Mode::Cautious,
        Origin::Shell,
        Policy::empty(),
    );
    assert_eq!(
        approval_reason(&users, Mode::Cautious).as_deref(),
        Some("A rule of yours in config.toml asks for this."),
    );
    let conversations = decide_with(
        &engine_with(Policy::empty()),
        readme(),
        Mode::Cautious,
        Origin::Shell,
        ask_reads(),
    );
    assert_eq!(
        approval_reason(&conversations, Mode::Cautious).as_deref(),
        Some("A rule of this conversation asks for this."),
    );
}

#[test]
fn the_floors_of_every_mode_say_why_they_ask() {
    // NOTE: a rule of the user that allows every command does not lift the floor.
    let commands =
        Policy::new(vec![Rule::new(Action::Execute, Resource::Any, Effect::Allow)]).unwrap();
    let sudo = decide_with(
        &engine_with(commands),
        shell("sudo pacman -Syu"),
        Mode::Cautious,
        Origin::Shell,
        Policy::empty(),
    );
    assert_eq!(
        approval_reason(&sudo, Mode::Cautious).as_deref(),
        Some("sudo runs commands as another user, so efr asks in every mode."),
    );
    assert_eq!(
        reason(
            Requirements::none().with_settings_change(SettingsChange::new("mode to auto", true)),
            Mode::Cautious,
        )
        .as_deref(),
        Some("Only the user approves a change of efr's settings."),
    );
}

#[test]
fn a_phone_turn_says_that_it_asks_outside_scratch() {
    let decision = decide_with(
        &engine_with(Policy::empty()),
        Requirements::none().with_write("/home/u/p/app/src/main.rs"),
        Mode::Auto,
        Origin::Phone,
        Policy::empty(),
    );
    assert_eq!(decision.effect(), Effect::Ask);
    let mode = efr_permissions::effective_mode(Mode::Auto, Origin::Phone);
    assert_eq!(
        approval_reason(&decision, mode).as_deref(),
        Some("A turn from the phone asks for everything outside $SCRATCH."),
    );
}

#[test]
fn an_exit_of_auto_names_what_the_sandbox_holds_back() {
    let decision = decide_with(
        &engine_with(Policy::empty()),
        shell("git push origin main").with_network(),
        Mode::Auto,
        Origin::Shell,
        Policy::empty(),
    );
    assert_eq!(decision.effect(), Effect::Ask, "{decision:?}");
    assert_eq!(
        approval_reason(&decision, Mode::Auto).as_deref(),
        Some("The auto sandbox holds back an upload with credentials."),
    );
}

#[test]
fn a_decision_that_does_not_ask_has_no_reason() {
    let decision = decide_with(
        &engine_with(Policy::empty()),
        Requirements::none().with_read("/home/u/p/app/README.md"),
        Mode::Cautious,
        Origin::Shell,
        Policy::empty(),
    );
    assert_eq!(decision.effect(), Effect::Allow);
    assert_eq!(approval_reason(&decision, Mode::Cautious), None);
}

#[test]
fn the_reason_is_one_line() {
    let decision = decide_with(
        &engine_with(Policy::empty()),
        Requirements::none().with_write("/home/u/notes\nnew"),
        Mode::Manual,
        Origin::Shell,
        Policy::empty(),
    );
    let reason = approval_reason(&decision, Mode::Manual).unwrap();
    assert!(!reason.contains('\n'), "{reason}");
}

#[test]
fn full_mode_says_what_it_still_asks_before() {
    let decision = decide_with(
        &engine_with(Policy::empty()),
        shell("rm -rf /home/u"),
        Mode::Full,
        Origin::Shell,
        Policy::empty(),
    );
    assert_eq!(decision.effect(), Effect::Ask, "{decision:?}");
    assert_eq!(
        approval_reason(&decision, Mode::Full).as_deref(),
        Some(
            "Full mode still asks before a delete of this tree, and only the user's own machine \
             answers."
        ),
    );
}
