//! The settle table of full mode (DF3), one reason at a time.

use std::path::PathBuf;

use efr_protocol::{ExitKind, ExitSource, Grant, Origin};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::settle;
use crate::decision::{Cause, Effect, Layer, Reason, Subject};
use crate::{CriticalDelete, CriticalTarget, CriticalWhy, ExitNeed};

/// Rules 0 to 6 of the machine are built in; 7 and later are the user's.
fn built_in(cause: &Cause) -> bool {
    matches!(
        cause,
        Cause::Rule { layer: Layer::Machine, index } | Cause::Part { layer: Layer::Machine, index, .. }
            if *index < 7
    )
}

fn reason(subject: Subject, effect: Effect, cause: Cause) -> Reason {
    Reason { subject, effect, cause }
}

fn command() -> Subject {
    Subject::Command { line: "make".to_owned(), deciding: Vec::new() }
}

fn exit(kind: ExitKind) -> Reason {
    let need = ExitNeed::new(kind, "make".to_owned(), ExitSource::Predicted);
    let effect = if kind.is_floor() { Effect::Deny } else { Effect::Ask };
    reason(Subject::Exit { need }, effect, Cause::Exit { kind })
}

fn settled(reason: Reason, job: bool) -> Vec<(Effect, Cause)> {
    settle(vec![reason], job, built_in)
        .into_iter()
        .map(|reason| (reason.effect, reason.cause))
        .collect()
}

#[rstest]
#[case::contained(Effect::Contain, Cause::Contained)]
#[case::exit_decides(Effect::Contain, Cause::ExitDecides)]
#[case::built_in_ask(Effect::Ask, Cause::Rule { layer: Layer::Machine, index: 0 })]
#[case::built_in_part(
    Effect::Ask,
    Cause::Part { layer: Layer::Machine, index: 0, part: "make".to_owned() }
)]
fn full_lets_the_sandbox_and_the_built_in_questions_run(
    #[case] effect: Effect,
    #[case] cause: Cause,
) {
    for job in [false, true] {
        assert_eq!(
            settled(reason(command(), effect, cause.clone()), job),
            [(Effect::Allow, Cause::Full)]
        );
    }
}

#[rstest]
#[case::user_ask(Effect::Ask, Cause::Rule { layer: Layer::Machine, index: 7 })]
#[case::user_deny(Effect::Deny, Cause::Rule { layer: Layer::Machine, index: 8 })]
#[case::conversation_ask(Effect::Ask, Cause::Rule { layer: Layer::Conversation, index: 0 })]
#[case::built_in_deny(Effect::Deny, Cause::Rule { layer: Layer::Machine, index: 5 })]
#[case::privileged(Effect::Ask, Cause::Privileged { program: "sudo".to_owned() })]
#[case::sealed(Effect::Deny, Cause::Sealed)]
#[case::write_sealed(Effect::Deny, Cause::WriteSealed)]
#[case::efr_own(Effect::Deny, Cause::EfrOwn)]
#[case::not_absolute(Effect::Deny, Cause::NotAbsolute)]
#[case::not_as_job(Effect::Deny, Cause::NotAsJob)]
#[case::settings(Effect::Ask, Cause::SettingsChange)]
#[case::remote_settings(Effect::Deny, Cause::RemoteSettings { origin: Origin::Phone })]
#[case::destructive(Effect::Ask, Cause::Destructive)]
#[case::user_allow(Effect::Allow, Cause::Rule { layer: Layer::Machine, index: 7 })]
fn full_keeps_floors_questions_and_the_users_rules(#[case] effect: Effect, #[case] cause: Cause) {
    assert_eq!(settled(reason(command(), effect, cause.clone()), false), [(effect, cause)]);
}

#[rstest]
#[case::privilege(ExitKind::Privilege)]
#[case::persistence(ExitKind::Persistence)]
#[case::destructive(ExitKind::Destructive)]
fn full_asks_for_privilege_persistence_and_an_uncovered_delete(#[case] kind: ExitKind) {
    assert_eq!(settled(exit(kind), false), [(Effect::Ask, Cause::Exit { kind })]);
}

#[rstest]
#[case::write(ExitKind::Write)]
#[case::host(ExitKind::Host)]
#[case::upload(ExitKind::Upload)]
#[case::masked_read(ExitKind::MaskedRead)]
#[case::synced_write(ExitKind::SyncedWrite)]
#[case::above_root(ExitKind::AboveRoot)]
#[case::bus(ExitKind::Bus)]
#[case::outside(ExitKind::Outside)]
fn full_runs_the_other_exits_and_keeps_them_for_the_record(#[case] kind: ExitKind) {
    let reasons = settle(vec![exit(kind)], false, built_in);
    assert_eq!(reasons.len(), 1);
    assert_eq!((reasons[0].effect, &reasons[0].cause), (Effect::Allow, &Cause::Exit { kind }));
    assert!(matches!(&reasons[0].subject, Subject::Exit { need } if need.kind == kind));
}

#[rstest]
#[case::secret(ExitKind::Secret)]
#[case::config(ExitKind::Config)]
fn full_keeps_the_floor_exits(#[case] kind: ExitKind) {
    assert_eq!(settled(exit(kind), false), [(Effect::Deny, Cause::Exit { kind })]);
    assert_eq!(settled(exit(kind), true), [(Effect::Deny, Cause::Exit { kind })]);
}

#[test]
fn a_destructive_exit_in_a_job_is_refused() {
    let reasons = settle(vec![exit(ExitKind::Destructive)], true, built_in);
    let found: Vec<(&Subject, Effect, &Cause)> =
        reasons.iter().map(|reason| (&reason.subject, reason.effect, &reason.cause)).collect();
    assert_eq!(found.len(), 2);
    assert_eq!(
        found[0],
        (
            &Subject::JobPart { part: "make".to_owned(), kind: ExitKind::Destructive },
            Effect::Deny,
            &Cause::NotAsJob
        )
    );
    assert_eq!(
        (found[1].1, found[1].2),
        (Effect::Ask, &Cause::Exit { kind: ExitKind::Destructive })
    );
}

#[test]
fn a_view_is_dropped() {
    let view = reason(Subject::View { grant: Grant::NetView }, Effect::Contain, Cause::Contained);
    assert_eq!(settle(vec![view], false, built_in), []);
}

#[test]
fn a_critical_delete_keeps_its_question() {
    let delete = CriticalDelete {
        part: "rm -rf ~".to_owned(),
        target: CriticalTarget::Path { path: PathBuf::from("/home/u"), why: CriticalWhy::Home },
    };
    let subject = Subject::CriticalDelete { delete };
    assert_eq!(
        settled(reason(subject.clone(), Effect::Ask, Cause::CriticalDelete), false),
        [(Effect::Ask, Cause::CriticalDelete)]
    );
    assert_eq!(
        settled(reason(subject, Effect::Deny, Cause::NotAsJob), true),
        [(Effect::Deny, Cause::NotAsJob)]
    );
}
