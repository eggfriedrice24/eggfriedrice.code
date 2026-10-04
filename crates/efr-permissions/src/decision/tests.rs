use std::path::PathBuf;

use efr_protocol::Origin;
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{Cause, Effect, Layer, Reason, Subject};
use crate::{Access, PathClass};

fn reason(subject: Subject, effect: Effect, cause: Cause) -> Reason {
    Reason { subject, effect, cause }
}

fn zshrc() -> Subject {
    Subject::Path {
        path: PathBuf::from("/home/u/.zshrc"),
        access: Access::Write,
        class: Some(PathClass::UserConfig),
    }
}

#[test]
fn effects_order_from_least_to_most_strict() {
    assert!(Effect::Allow < Effect::Ask);
    assert!(Effect::Ask < Effect::Deny);
    assert_eq!([Effect::Ask, Effect::Deny, Effect::Allow].into_iter().max(), Some(Effect::Deny));
}

#[rstest]
#[case::machine_rule(
    reason(zshrc(), Effect::Ask, Cause::Rule { layer: Layer::Machine, index: 4 }),
    "write /home/u/.zshrc (user config): ask, by rule 4 of the machine policy"
)]
#[case::conversation_rule(
    reason(zshrc(), Effect::Allow, Cause::Rule { layer: Layer::Conversation, index: 0 }),
    "write /home/u/.zshrc (user config): allow, by rule 0 of the conversation policy"
)]
#[case::remote(
    reason(zshrc(), Effect::Ask, Cause::RemoteOrigin { origin: Origin::Phone }),
    "write /home/u/.zshrc (user config): ask, because the turn comes from the phone and is outside $SCRATCH"
)]
#[case::no_rule(
    reason(Subject::Network, Effect::Deny, Cause::NoRule),
    "network access: deny, because no rule matched"
)]
#[case::relative(
    reason(
        Subject::Path { path: PathBuf::from("notes.txt"), access: Access::Read, class: None },
        Effect::Deny,
        Cause::NotAbsolute
    ),
    "read notes.txt: deny, because the path is not absolute"
)]
#[case::command(
    reason(
        Subject::Command { line: "ls -la".to_owned() },
        Effect::Ask,
        Cause::Rule { layer: Layer::Machine, index: 0 }
    ),
    "run \"ls -la\": ask, by rule 0 of the machine policy"
)]
#[case::interactive(
    reason(Subject::Interactive, Effect::Ask, Cause::Interactive),
    "input at the terminal: ask, because the user must answer at the terminal"
)]
#[case::nothing(
    reason(Subject::Nothing, Effect::Allow, Cause::NoRequirements),
    "no requirements: allow"
)]
fn reasons_read_as_one_line(#[case] reason: Reason, #[case] expected: &str) {
    assert_eq!(reason.to_string(), expected);
}
