use std::path::{Path, PathBuf};

use efr_protocol::{
    AttachHow, NewReason, ProjectId, RepoHint, RouteLabel, RouteScope, Routed, Switch,
};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use super::{age, attached, hint_line, line, scope_mark};

const HOME: &str = "/home/u";

fn now() -> Timestamp {
    "2026-10-20T12:00:00Z".parse().unwrap()
}

fn ago(minutes: i64) -> Timestamp {
    now() - jiff::SignedDuration::from_mins(minutes)
}

fn app() -> RouteLabel {
    RouteLabel {
        scope: RouteScope::Project(
            "0192f0c1-7a00-7000-8000-0000000a4401".parse::<ProjectId>().unwrap(),
        ),
        name: Some("app".to_owned()),
        dir: PathBuf::from("/home/u/p/app"),
    }
}

fn outside(dir: &str) -> RouteLabel {
    RouteLabel { scope: RouteScope::Outside, name: None, dir: PathBuf::from(dir) }
}

fn routed(scope: RouteLabel, switch: Option<Switch>) -> Routed {
    Routed { scope, switch, hint: None }
}

fn shown(route: &Routed, note: Option<&str>) -> String {
    line(route, note, now(), Some(Path::new(HOME)))
}

#[test]
fn switch_line_back() {
    let back = Switch::Back { title: Some("fix the tests".to_owned()), updated_at: ago(125) };
    insta::assert_snapshot!(shown(&routed(app(), Some(back)), None), @r#"↪ back to "fix the tests" in app (2 h ago)"#);
}

#[test]
fn switch_line_new_from_home() {
    let new = Switch::New { from: Some(outside(HOME)), reason: NewReason::Scope };
    insta::assert_snapshot!(shown(&routed(app(), Some(new)), None), @"↪ new conversation for app (from ~)");
    let from_app = Switch::New { from: Some(app()), reason: NewReason::Scope };
    insta::assert_snapshot!(
        shown(&routed(outside("/etc"), Some(from_app)), None),
        @"↪ new conversation outside projects (from app)"
    );
}

#[test]
fn switch_line_picked_up() {
    let picked =
        Switch::PickedUp { title: Some("fix the proxy test".to_owned()), updated_at: ago(25) };
    insta::assert_snapshot!(
        shown(&routed(app(), Some(picked)), None),
        @r#"↪ picked up "fix the proxy test" in app (25 min ago); ,new starts fresh"#
    );
}

#[test]
fn switch_line_new_after_idle() {
    let idle = Switch::New { from: None, reason: NewReason::Idle { hours: 13 } };
    insta::assert_snapshot!(
        shown(&routed(app(), Some(idle)), None),
        @"↪ new conversation for app (the last one was idle for 13 h)"
    );
    let moved = Switch::New {
        from: None,
        reason: NewReason::Moved {
            tty: "/dev/pts/7".to_owned(),
            title: Some("fix the tests".to_owned()),
        },
    };
    insta::assert_snapshot!(
        shown(&routed(app(), Some(moved)), None),
        @r#"↪ new conversation for app ("fix the tests" moved to /dev/pts/7)"#
    );
}

#[test]
fn head_line_project() {
    insta::assert_snapshot!(shown(&routed(app(), None), None), @"app");
}

#[test]
fn head_line_no_project() {
    insta::assert_snapshot!(shown(&routed(outside("/home/u/notes"), None), None), @"○ no project · ~/notes");
    insta::assert_snapshot!(shown(&routed(outside("/etc"), None), None), @"○ no project · /etc");
}

#[test]
fn head_line_joins_the_settings_note() {
    insta::assert_snapshot!(
        shown(&routed(app(), None), Some("mode auto, model gpt-5.4")),
        @"app · mode auto, model gpt-5.4"
    );
    let back = Switch::Back { title: None, updated_at: ago(3) };
    insta::assert_snapshot!(
        shown(&routed(outside(HOME), Some(back)), Some("mode auto")),
        @"↪ back to a conversation with no title outside projects (3 min ago) · mode auto"
    );
}

#[test]
fn a_title_cannot_drive_the_terminal() {
    let back =
        Switch::Back { title: Some("evil\u{1b}]52;c;x\u{7}\nline".to_owned()), updated_at: ago(1) };
    let text = shown(&routed(app(), Some(back)), None);
    assert!(!text.contains('\u{1b}') && !text.contains('\n'), "{text:?}");
}

#[test]
fn ages_are_one_coarse_unit() {
    assert_eq!(age(now(), now()), "just now");
    assert_eq!(age(ago(25), now()), "25 min ago");
    assert_eq!(age(ago(13 * 60 + 5), now()), "13 h ago");
    assert_eq!(age(ago(3 * 24 * 60), now()), "3 d ago");
}

#[test]
fn history_names_the_terminal_of_an_attach() {
    assert_eq!(attached("/dev/pts/7", AttachHow::PickedUp), "picked up in /dev/pts/7");
    assert_eq!(attached("/dev/pts/7", AttachHow::Resumed), "resumed in /dev/pts/7");
}

#[test]
fn switch_line_new_from_outside_inside_the_project() {
    // The prompts in ~/p/lib went outside projects while `follow_project` was off; when
    // it is on again, the new lib conversation comes from the outside one.
    let lib = RouteLabel {
        scope: RouteScope::Project(
            "0192f0c1-7a00-7000-8000-0000000a4402".parse::<ProjectId>().unwrap(),
        ),
        name: Some("lib".to_owned()),
        dir: PathBuf::from("/home/u/p/lib"),
    };
    let new = Switch::New { from: Some(outside("/home/u/p/lib/src")), reason: NewReason::Scope };
    insta::assert_snapshot!(
        shown(&routed(lib, Some(new)), None),
        @"↪ new conversation for lib (from outside projects)"
    );
}

#[test]
fn repo_hint_line() {
    let home = Some(Path::new(HOME));
    let tool = RepoHint { root: PathBuf::from("/home/u/p/tool") };
    insta::assert_snapshot!(
        hint_line(&tool, home),
        @"○ ~/p/tool is a git repo but not a project: writes there ask you. efr project add ~/p/tool registers it."
    );
    // A path that is no plain shell word shows in full and in quotes, so the command can
    // be pasted.
    let spaced = RepoHint { root: PathBuf::from("/home/u/my tool's") };
    insta::assert_snapshot!(
        hint_line(&spaced, home),
        @r#"○ ~/my tool's is a git repo but not a project: writes there ask you. efr project add '/home/u/my tool'\''s' registers it."#
    );
    let odd = RepoHint { root: PathBuf::from("/srv/evil\u{1b}]52;c;x\u{7}\nrepo") };
    let text = hint_line(&odd, home);
    assert!(!text.contains('\u{1b}') && !text.contains('\n'), "{text:?}");
}

#[test]
fn the_scope_mark_of_the_status_row() {
    let home = Some(Path::new(HOME));
    assert_eq!(scope_mark(&app(), home), "app");
    assert_eq!(scope_mark(&outside(HOME), home), "○ ~");
    assert_eq!(scope_mark(&outside("/etc"), home), "○ /etc");
    let removed = RouteLabel { name: None, ..app() };
    assert_eq!(scope_mark(&removed, home), "removed project");
}
