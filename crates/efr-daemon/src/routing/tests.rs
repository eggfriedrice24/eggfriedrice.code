use std::path::{Path, PathBuf};
use std::time::Duration;

use efr_protocol::{
    ConversationId, ConversationStatus, NewReason, ProjectId, RouteLabel, RouteScope, Switch,
};
use efr_scope::Registry;
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use super::{
    Before, Candidate, Outcome, Verdict, continues, idle_hours, idle_limit, label, may_pick_up,
    pick_up, route_key, slot_key, switch,
};

fn conversation(n: u128) -> ConversationId {
    ConversationId::from_uuid(uuid::Uuid::from_u128(n))
}

fn project(n: u128) -> ProjectId {
    ProjectId::from_uuid(uuid::Uuid::from_u128(n))
}

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

const NOW: &str = "2026-10-04T12:00:00Z";
const TWELVE_HOURS: Option<Duration> = Some(Duration::from_secs(12 * 3600));

fn registry() -> Registry {
    let mut registry = Registry::empty();
    registry.register(project(1), PathBuf::from("/home/u/p/app"), Some("app".to_owned())).unwrap();
    registry.register(project(2), PathBuf::from("/home/u/p/app/vendor/lib"), None).unwrap();
    registry
}

#[test]
fn the_key_is_the_deepest_project_of_the_path_or_its_resolved_form() {
    let registry = registry();
    let key = |pwd: &str, resolved: Option<&str>| {
        route_key(&registry, Path::new(pwd), resolved.map(Path::new))
    };
    assert_eq!(key("/home/u/p/app", None), RouteScope::Project(project(1)));
    assert_eq!(key("/home/u/p/app/src/bin", None), RouteScope::Project(project(1)));
    assert_eq!(key("/home/u/p/app/vendor/lib/x", None), RouteScope::Project(project(2)));
    assert_eq!(key("/home/u/p/application", None), RouteScope::Outside);
    assert_eq!(key("/home/u", None), RouteScope::Outside);
    assert_eq!(key("/etc", None), RouteScope::Outside);
    assert_eq!(key("/home/u/link", Some("/home/u/p/app/src")), RouteScope::Project(project(1)));
}

#[test]
fn a_label_names_the_project_or_the_directory() {
    let registry = registry();
    assert_eq!(
        label(&registry, &RouteScope::Project(project(1)), Path::new("/home/u/p/app/src")),
        RouteLabel {
            scope: RouteScope::Project(project(1)),
            name: Some("app".to_owned()),
            dir: "/home/u/p/app".into(),
        }
    );
    let unnamed = label(&registry, &RouteScope::Project(project(2)), Path::new("/x"));
    assert_eq!(unnamed.name.as_deref(), Some("lib"), "the last part of the root");
    let removed = label(&registry, &RouteScope::Project(project(9)), Path::new("/x"));
    assert_eq!((removed.name, removed.dir), (None, PathBuf::from("/x")));
    let outside = label(&registry, &RouteScope::Outside, Path::new("/etc/nixos"));
    assert_eq!((outside.name, outside.dir), (None, PathBuf::from("/etc/nixos")));
}

#[test]
fn own_slot_continues_within_the_idle_time() {
    let recent = Some(at("2026-10-04T11:00:00Z"));
    let never = |_| panic!("the same shell needs no liveness check");

    let verdict = |holder, asking| continues(holder, asking, recent, at(NOW), TWELVE_HOURS, never);
    assert_eq!(verdict(Some(41), Some(41)), Verdict::Continues);
    // A shell or a record that does not say its pid continues as before.
    assert_eq!(verdict(None, Some(41)), Verdict::Continues);
    assert_eq!(verdict(Some(41), None), Verdict::Continues);
}

#[test]
fn idle_slot_starts_new_while_its_shell_lives() {
    let alive = |_| true;
    let old = Some(at("2026-10-03T23:59:59Z"));
    let young = Some(at("2026-10-04T00:00:01Z"));

    assert_eq!(continues(Some(41), Some(41), old, at(NOW), TWELVE_HOURS, alive), Verdict::Idle);
    assert_eq!(
        continues(Some(41), Some(41), young, at(NOW), TWELVE_HOURS, alive),
        Verdict::Continues
    );
    // 0 hours in the config: never idle.
    assert_eq!(
        continues(Some(41), Some(41), old, at(NOW), idle_limit(0), alive),
        Verdict::Continues
    );
    // A nested shell in the same terminal while the first still runs.
    assert_eq!(
        continues(Some(41), Some(77), young, at(NOW), TWELVE_HOURS, alive),
        Verdict::Continues
    );
}

#[test]
fn shell_gone_is_checked_before_idle() {
    let old = Some(at("2026-10-01T00:00:00Z"));
    let gone = continues(Some(41), Some(77), old, at(NOW), TWELVE_HOURS, |pid| {
        assert_eq!(pid, 41);
        false
    });
    assert_eq!(gone, Verdict::ShellGone, "a new tab on a reused pts, however idle (F21)");
}

fn candidate(status: ConversationStatus, updated_at: &str, held: bool) -> Candidate {
    Candidate { id: conversation(5), status, updated_at: at(updated_at), held }
}

#[test]
fn pick_up_takes_only_the_single_newest() {
    let newest = candidate(ConversationStatus::Idle, "2026-10-04T11:35:00Z", false);
    assert_eq!(pick_up(Some(&newest), at(NOW), TWELVE_HOURS), Some(conversation(5)));
    // The newest is held: no fall back to an older one, which the caller never reads.
    let held = Candidate { held: true, ..newest };
    assert_eq!(pick_up(Some(&held), at(NOW), TWELVE_HOURS), None);
    assert_eq!(pick_up(None, at(NOW), TWELVE_HOURS), None);
    let stale = candidate(ConversationStatus::Idle, "2026-10-03T23:00:00Z", false);
    assert_eq!(pick_up(Some(&stale), at(NOW), TWELVE_HOURS), None, "older than the idle time");
}

#[test]
fn pick_up_skips_a_running_or_waiting_conversation() {
    for status in [ConversationStatus::Running, ConversationStatus::AwaitingApproval] {
        let busy = candidate(status, "2026-10-04T11:59:00Z", false);
        assert_eq!(pick_up(Some(&busy), at(NOW), TWELVE_HOURS), None, "{status:?}");
    }
}

#[test]
fn pick_up_window_zero_has_no_age_limit() {
    let ancient = candidate(ConversationStatus::Idle, "2025-01-01T00:00:00Z", false);
    assert_eq!(pick_up(Some(&ancient), at(NOW), idle_limit(0)), Some(conversation(5)));
}

#[test]
fn pick_up_never_in_the_outside_scope() {
    assert!(!may_pick_up(&RouteScope::Outside, true, true));
    assert!(!may_pick_up(&RouteScope::Unknown, true, true));
    assert!(may_pick_up(&RouteScope::Project(project(1)), true, true));
    assert!(!may_pick_up(&RouteScope::Project(project(1)), false, true), "follow_project off");
    assert!(!may_pick_up(&RouteScope::Project(project(1)), true, false), "no working directory");
}

#[test]
fn follow_project_off_routes_to_the_current_slot() {
    let app = RouteScope::Project(project(1));
    let outside = RouteScope::Outside;
    assert_eq!(slot_key(Some(&outside), &app, false, true, false), outside);
    assert_eq!(slot_key(Some(&outside), &app, true, true, false), app, "on: the pwd's key");
    assert_eq!(slot_key(Some(&outside), &app, false, true, true), app, ",new takes the pwd's key");
    assert_eq!(slot_key(None, &app, false, true, false), app, "a terminal's first prompt");
    assert_eq!(slot_key(Some(&outside), &app, true, false, false), outside, "no pwd");
}

fn home() -> RouteLabel {
    RouteLabel { scope: RouteScope::Outside, name: None, dir: "/home/u".into() }
}

#[test]
fn switch_back_names_title_scope_and_age() {
    let before = Before { conversation_id: conversation(1), label: home() };
    let back = Outcome::Continued {
        conversation_id: conversation(2),
        title: Some("fix the tests".to_owned()),
        updated_at: at("2026-10-04T10:00:00Z"),
    };
    assert_eq!(
        switch(Some(&before), back),
        Some(Switch::Back {
            title: Some("fix the tests".to_owned()),
            updated_at: at("2026-10-04T10:00:00Z"),
        })
    );
    let same = Outcome::Continued {
        conversation_id: conversation(1),
        title: None,
        updated_at: at("2026-10-04T10:00:00Z"),
    };
    assert_eq!(switch(Some(&before), same), None, "the same conversation is no switch");
}

#[test]
fn switch_new_names_the_scope_it_came_from() {
    let before = Before { conversation_id: conversation(1), label: home() };
    assert_eq!(
        switch(Some(&before), Outcome::New),
        Some(Switch::New { from: Some(home()), reason: NewReason::Scope })
    );
    assert_eq!(
        switch(Some(&before), Outcome::Idle { hours: 13 }),
        Some(Switch::New { from: None, reason: NewReason::Idle { hours: 13 } })
    );
    assert_eq!(idle_hours(at("2026-10-03T22:59:00Z"), at(NOW)), 13);
}

#[test]
fn first_prompt_of_a_terminal_has_no_switch() {
    assert_eq!(switch(None, Outcome::New), None);
}

#[test]
fn comma_new_has_no_switch() {
    let before = Before { conversation_id: conversation(1), label: home() };
    assert_eq!(switch(Some(&before), Outcome::Fresh), None);
}
