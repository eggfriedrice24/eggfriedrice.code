use std::path::{Path, PathBuf};

use efr_protocol::{
    AttachHow, NewReason, ProjectId, RepoHint, RouteLabel, RouteScope, Switch, SwitchedModel,
};
use efr_render::Glyphs;
use efr_test_support::design::{Depth, Frame, assert_rows};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use super::{Pieces, age, attached, hint_line, model_line, scope_mark, switch_line, wake_line};
use crate::format::ending::{plain, rows};
use crate::testing::design_options;

const HOME: &str = "/home/u";

fn now() -> Timestamp {
    "2026-10-20T12:00:00Z".parse().unwrap()
}

fn ago(minutes: i64) -> Timestamp {
    now() - jiff::SignedDuration::from_mins(minutes)
}

fn project(name: &str) -> RouteLabel {
    RouteLabel {
        scope: RouteScope::Project(
            "0192f0c1-7a00-7000-8000-0000000a4401".parse::<ProjectId>().unwrap(),
        ),
        name: Some(name.to_owned()),
        dir: PathBuf::from(format!("/home/u/p/{name}")),
    }
}

fn app() -> RouteLabel {
    project("app")
}

fn outside(dir: &str) -> RouteLabel {
    RouteLabel { scope: RouteScope::Outside, name: None, dir: PathBuf::from(dir) }
}

fn line(switch: &Switch, scope: &RouteLabel) -> Pieces {
    switch_line(switch, scope, now(), Some(Path::new(HOME)), &Glyphs::UNICODE)
}

fn shown(switch: &Switch, scope: &RouteLabel) -> String {
    plain(&line(switch, scope))
}

fn side(provider: Option<&str>, model: &str, effort: Option<&str>) -> SwitchedModel {
    SwitchedModel {
        provider: provider.map(str::to_owned),
        model: model.to_owned(),
        effort: effort.map(str::to_owned),
    }
}

fn work() -> SwitchedModel {
    side(Some("anthropic-api.work"), "claude-opus-5-5", Some("high"))
}

fn personal() -> SwitchedModel {
    side(Some("anthropic-api.personal"), "claude-opus-5-5", Some("high"))
}

fn gpt_b() -> SwitchedModel {
    side(Some("openai-subscription.b"), "gpt-5.5", Some("medium"))
}

fn back(title: &str, minutes: i64) -> Switch {
    Switch::Back { title: Some(title.to_owned()), updated_at: ago(minutes) }
}

fn new_from(from: RouteLabel) -> Switch {
    Switch::New { from: Some(from), reason: NewReason::Scope }
}

fn idle(hours: u64) -> Switch {
    Switch::New { from: None, reason: NewReason::Idle { hours } }
}

fn moved(title: &str, tty: &str) -> Switch {
    Switch::New {
        from: None,
        reason: NewReason::Moved { tty: tty.to_owned(), title: Some(title.to_owned()) },
    }
}

fn picked(title: &str, minutes: i64) -> Switch {
    Switch::PickedUp { title: Some(title.to_owned()), updated_at: ago(minutes) }
}

/// The lines of TM-Scope frame 19 that the turn slice writes, by row: the `↪` lines,
/// the `⇄` lines and the repo hint. Row 7 (`↪ resumed ...`) is the line of `,resume`,
/// which `format/resume.rs` writes.
fn scope_19() -> Vec<(usize, Pieces)> {
    let glyphs = &Glyphs::UNICODE;
    let tool = RepoHint { root: PathBuf::from("/home/u/src/tool") };
    vec![
        (0, line(&back("fix steer race", 125), &app())),
        (1, line(&back("nftables rules", 125), &outside("/etc/nftables.d"))),
        (2, line(&new_from(outside(HOME)), &app())),
        (3, line(&new_from(app()), &outside(HOME))),
        (4, line(&idle(13), &app())),
        (5, line(&moved("fix the tests", "/dev/pts/7"), &app())),
        (6, line(&picked("fix the proxy test", 25), &app())),
        (8, model_line(&work(), &gpt_b(), Some((86_000, 6_000)), glyphs)),
        (9, model_line(&work(), &personal(), None, glyphs)),
        (10, hint_line(&tool, Some(Path::new(HOME)), glyphs)),
    ]
}

/// TM-Scope frame 19: each event line in its roles (the mark in `accent`, the sentence
/// in `text`, the rest muted, `,new` and the command of the hint in `code`), in the
/// three depths.
#[test]
fn the_event_lines_are_the_scope_board() {
    let frame = Frame::load("TM-Scope", 19).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        for (row, pieces) in scope_19() {
            assert_rows(&frame, row..=row, &rows(&pieces, 0, &options), depth);
        }
    }
}

/// TM-Legend frame 4 at its 44 columns, and the `↪` and `⇄` rows of TM-Endings 2 to 7
/// in the three depths.
#[test]
fn the_event_lines_are_the_legend_and_the_endings_board() {
    let legend = Frame::load("TM-Legend", 4).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, legend.cols);
        let home = line(&back("nftables rules", 125), &outside(HOME));
        assert_rows(&legend, .., &rows(&home, 0, &options), depth);
    }

    let glyphs = &Glyphs::UNICODE;
    let steer_race = line(&back("fix steer race", 125), &app());
    let cases: [(u32, usize, Pieces); 6] = [
        (2, 1, steer_race.clone()),
        (3, 1, line(&new_from(app()), &outside(HOME))),
        (4, 1, line(&picked("fix the proxy test", 25), &app())),
        (5, 3, model_line(&work(), &gpt_b(), Some((86_400, 6_050)), glyphs)),
        (6, 1, model_line(&work(), &personal(), None, glyphs)),
        (7, 2, model_line(&work(), &personal(), None, glyphs)),
    ];
    for (number, row, pieces) in cases {
        let frame = Frame::load("TM-Endings", number).unwrap();
        for depth in Depth::ALL {
            let options = design_options(depth, frame.cols);
            assert_rows(&frame, row..=row, &rows(&pieces, 0, &options), depth);
        }
    }
}

#[test]
fn switch_lines_say_what_changed_in_words() {
    assert_eq!(
        shown(&back("fix the tests", 125), &app()),
        r#"↪ back to "fix the tests" in app (2 h ago)"#
    );
    assert_eq!(
        shown(&Switch::Back { title: None, updated_at: ago(3) }, &outside(HOME)),
        "↪ back to a conversation with no title at home (3 min ago)"
    );
    assert_eq!(shown(&new_from(outside(HOME)), &app()), "↪ new conversation for app (from home)");
    assert_eq!(shown(&new_from(app()), &outside("/etc")), "↪ new conversation at home (from app)");
    assert_eq!(
        shown(&picked("fix the proxy test", 25), &app()),
        r#"↪ picked up "fix the proxy test" in app (25 min ago) · ,new starts fresh"#
    );
    assert_eq!(
        shown(&idle(13), &app()),
        "↪ new conversation for app (the last one was idle for 13 h)"
    );
    assert_eq!(
        shown(&moved("fix the tests", "/dev/pts/7"), &app()),
        r#"↪ new conversation for app ("fix the tests" moved to /dev/pts/7)"#
    );
    // A project that is no longer registered.
    let removed = RouteLabel { name: None, ..app() };
    assert_eq!(shown(&back("x", 60), &removed), r#"↪ back to "x" in a removed project (1 h ago)"#);
    assert_eq!(
        shown(&new_from(removed), &outside(HOME)),
        "↪ new conversation at home (from ~/p/app)"
    );
    // A kind of switch from a newer daemon still says that the conversation changed.
    assert_eq!(shown(&Switch::Unknown, &app()), "↪ another conversation in app");
}

#[test]
fn switch_lines_have_an_ascii_form() {
    let ascii = switch_line(&picked("t", 25), &app(), now(), Some(Path::new(HOME)), &Glyphs::ASCII);
    assert_eq!(plain(&ascii), r#"=> picked up "t" in app (25 min ago) - ,new starts fresh"#);
    let hint = hint_line(
        &RepoHint { root: PathBuf::from("/home/u/p/tool") },
        Some(Path::new(HOME)),
        &Glyphs::ASCII,
    );
    assert!(plain(&hint).starts_with("~ ~/p/tool is a git repo"), "{}", plain(&hint));
}

#[test]
fn a_new_conversation_inside_a_project_from_home_says_from_home() {
    // The prompts in ~/p/lib went to home while `follow_project` was off; when it is on
    // again, the new lib conversation comes from home.
    let new = new_from(outside("/home/u/p/lib/src"));
    assert_eq!(shown(&new, &project("lib")), "↪ new conversation for lib (from home)");
}

#[test]
fn a_title_cannot_drive_the_terminal() {
    let evil = back("evil\u{1b}]52;c;x\u{7}\nline", 1);
    let text = shown(&evil, &app());
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
fn repo_hint_line() {
    let home = Some(Path::new(HOME));
    let glyphs = &Glyphs::UNICODE;
    let tool = RepoHint { root: PathBuf::from("/home/u/p/tool") };
    assert_eq!(
        plain(&hint_line(&tool, home, glyphs)),
        "⌂ ~/p/tool is a git repo but not a project: writes there ask you. efr project add ~/p/tool"
    );
    // A path that is no plain shell word shows in full and in quotes, so the command can
    // be pasted.
    let spaced = RepoHint { root: PathBuf::from("/home/u/my tool's") };
    assert_eq!(
        plain(&hint_line(&spaced, home, glyphs)),
        r#"⌂ ~/my tool's is a git repo but not a project: writes there ask you. efr project add '/home/u/my tool'\''s'"#
    );
    let odd = RepoHint { root: PathBuf::from("/srv/evil\u{1b}]52;c;x\u{7}\nrepo") };
    let text = plain(&hint_line(&odd, home, glyphs));
    assert!(!text.contains('\u{1b}') && !text.contains('\n'), "{text:?}");
}

#[test]
fn the_scope_mark_of_the_status_row() {
    let home = Some(Path::new(HOME));
    let glyphs = &Glyphs::UNICODE;
    assert_eq!(scope_mark(&app(), home, glyphs), "app");
    assert_eq!(scope_mark(&outside(HOME), home, glyphs), "⌂ ~");
    assert_eq!(scope_mark(&outside("/etc"), home, glyphs), "⌂ /etc");
    assert_eq!(scope_mark(&outside("/etc"), home, &Glyphs::ASCII), "~ /etc");
    let removed = RouteLabel { name: None, ..app() };
    assert_eq!(scope_mark(&removed, home, glyphs), "removed project");
}

#[test]
fn a_switch_names_both_sides_as_model_takes_them() {
    let glyphs = &Glyphs::UNICODE;
    assert_eq!(
        plain(&model_line(&work(), &gpt_b(), None, glyphs)),
        "⇄ b/gpt-5.5 medium  was work/claude-opus-5-5 high"
    );
    // A provider with no account gives the model alone, and no effort is no word.
    let api = side(Some("openai-api"), "gpt-5.5", None);
    let old = side(None, "gpt-5.4", None);
    assert_eq!(plain(&model_line(&old, &api, None, glyphs)), "⇄ gpt-5.5  was gpt-5.4");
    // The same model on another provider starts with a cold cache.
    let subscription = side(Some("openai-subscription"), "gpt-5.5", None);
    assert_eq!(
        plain(&model_line(&subscription, &api, None, &Glyphs::ASCII)),
        "<> gpt-5.5  was gpt-5.5 - cache starts cold"
    );
    // A compaction that came first stands on the line, in place of the cold cache.
    assert_eq!(
        plain(&model_line(&subscription, &api, Some((231_000, 24_000)), glyphs)),
        "⇄ gpt-5.5  was gpt-5.5 · context compacted 231k → 24k"
    );
}

#[test]
fn a_job_notice_marks_its_turn_without_an_emoji() {
    assert_eq!(
        plain(&wake_line(&Glyphs::UNICODE)),
        "↪ a job notice started this turn (not your input)"
    );
    assert_eq!(
        plain(&wake_line(&Glyphs::ASCII)),
        "=> a job notice started this turn (not your input)"
    );
}
