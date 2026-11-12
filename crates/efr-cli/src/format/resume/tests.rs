use std::path::{Path, PathBuf};

use efr_protocol::{
    ConversationId, ConversationStatus, ConversationSummary, ConversationsRecentResult, ProjectId,
    RecentConversation, RouteLabel, RouteScope, Seq,
};
use efr_render::{Glyphs, RenderOptions};
use efr_test_support::design::{Depth, Frame, assert_rows};
use jiff::Timestamp;
use pretty_assertions::assert_eq;

use super::{Resumed, held_question, list, resumed_line};
use crate::testing::{design_options, readable};

const HOME: &str = "/home/u";

fn now() -> Timestamp {
    "2026-10-20T12:00:00Z".parse().unwrap()
}

fn ago(minutes: i64) -> Timestamp {
    now() - jiff::SignedDuration::from_mins(minutes)
}

fn app() -> RouteScope {
    RouteScope::Project("0192f0c1-7a00-7000-8000-0000000a4401".parse::<ProjectId>().unwrap())
}

fn app_label() -> RouteLabel {
    RouteLabel { scope: app(), name: Some("app".to_owned()), dir: PathBuf::from("/home/u/p/app") }
}

fn row(
    n: u128,
    title: Option<&str>,
    route: RouteScope,
    project: Option<&str>,
    minutes: i64,
    turns: u32,
) -> RecentConversation {
    RecentConversation {
        summary: ConversationSummary {
            id: format!("019a9b1c-3d00-7a10-8b20-{n:012x}").parse::<ConversationId>().unwrap(),
            title: title.map(str::to_owned),
            status: ConversationStatus::Idle,
            created_at: ago(minutes + 60),
            updated_at: ago(minutes),
            last_seq: Seq::new(1),
            cwd: None,
            scope: None,
            tty: None,
        },
        route,
        project: project.map(str::to_owned),
        turns,
        open_in: None,
        here: false,
    }
}

fn recent() -> ConversationsRecentResult {
    let removed =
        RouteScope::Project("0192f0c1-7a00-7000-8000-0000000a4409".parse::<ProjectId>().unwrap());
    ConversationsRecentResult {
        scope: Some(app_label()),
        current: None,
        conversations: vec![
            RecentConversation {
                here: true,
                ..row(1, Some("fix the proxy test"), app(), Some("app"), 25, 6)
            },
            RecentConversation {
                open_in: Some("/dev/pts/4".to_owned()),
                ..row(2, Some("why is the build slow"), app(), Some("app"), 125, 3)
            },
            row(
                3,
                Some("clean up the nixos config and the old home manager modules of the laptop"),
                RouteScope::Outside,
                None,
                1_500,
                12,
            ),
            row(4, None, removed, None, 4_400, 1),
        ],
    }
}

#[test]
fn resume_list_at_80_columns() {
    let options = RenderOptions::new(80);
    let text = list(&recent(), now(), true, &options);
    insta::assert_snapshot!(readable(&text));
    for line in crate::testing::bare(&text).lines() {
        assert!(line.chars().count() <= 80, "{line}");
    }
}

#[test]
fn resume_list_in_a_pipe() {
    let options = RenderOptions::new(80).with_terminal(false);
    let text = list(&recent(), now(), false, &options);
    insta::assert_snapshot!(text);
    assert!(!text.contains('\x1b'), "no colours in a pipe");
    assert!(text.contains("home manager modules of the laptop"), "no cut in a pipe");
}

#[test]
fn an_empty_list_says_so() {
    let empty = ConversationsRecentResult::default();
    assert_eq!(list(&empty, now(), true, &RenderOptions::new(80)), "no conversations yet\n");
}

#[test]
fn resume_line_in_another_scope() {
    let home = Some(Path::new(HOME));
    let label = app_label();
    let plain = RenderOptions::new(80).with_terminal(false);
    let line = |here, continues_here| {
        let resumed = Resumed {
            title: Some("fix the tests"),
            scope: &label,
            updated_at: ago(125),
            here,
            continues_here,
        };
        resumed_line(&resumed, now(), home, &plain)
    };
    insta::assert_snapshot!(
        line(false, false),
        @r#"↪ resumed "fix the tests" (app, 2 h ago); it continues at your next prompt in ~/p/app"#
    );
    insta::assert_snapshot!(line(false, true), @r#"↪ resumed "fix the tests" (app, 2 h ago)"#);
    insta::assert_snapshot!(line(true, true), @r#"↪ "fix the tests" is this terminal's app conversation"#);
    let outside = RouteLabel { scope: RouteScope::Outside, name: None, dir: PathBuf::from(HOME) };
    let resumed = Resumed {
        title: None,
        scope: &outside,
        updated_at: ago(3),
        here: true,
        continues_here: true,
    };
    insta::assert_snapshot!(
        resumed_line(&resumed, now(), home, &plain),
        @"↪ the conversation with no title is this terminal's home conversation"
    );
}

/// TM-Scope frame 19, row 7: the line after a resume of the home conversation from a
/// project, in the three depths.
#[test]
fn the_line_after_a_resume_is_the_design() {
    let frame = Frame::load("TM-Scope", 19).unwrap();
    let outside = RouteLabel { scope: RouteScope::Outside, name: None, dir: PathBuf::from(HOME) };
    let resumed = Resumed {
        title: Some("nftables rules"),
        scope: &outside,
        updated_at: ago(125),
        here: false,
        continues_here: false,
    };
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let line = format!("{}\n", resumed_line(&resumed, now(), Some(Path::new(HOME)), &options));
        assert_rows(&frame, 7..=7, &line, depth);
    }
}

/// The three conversations of TM-Scope frame 11: one of `app` that this terminal holds,
/// the home conversation, and one of a project named `home`.
fn scope_rows() -> ConversationsRecentResult {
    let named_home =
        RouteScope::Project("0192f0c1-7a00-7000-8000-0000000a4402".parse::<ProjectId>().unwrap());
    ConversationsRecentResult {
        scope: Some(app_label()),
        current: None,
        conversations: vec![
            RecentConversation {
                here: true,
                ..row(1, Some("fix steer race in turn loop"), app(), Some("app"), 25, 6)
            },
            row(2, Some("nftables rules"), RouteScope::Outside, None, 125, 5),
            row(3, Some("why is the build slow"), named_home, Some("home"), 3 * 1_440 + 5, 2),
        ],
    }
}

/// TM-Scope frame 11, rows 8 to 10: the resume list with the column `⌂ home`, so a
/// project named `home` stays a bare `home` beside it. The frame draws the rows alone;
/// the line that says how to resume one follows them.
#[test]
fn the_resume_list_is_the_design() {
    let frame = Frame::load("TM-Scope", 11).unwrap();
    for depth in Depth::ALL {
        let options = design_options(depth, frame.cols);
        let text = list(&scope_rows(), now(), true, &options);
        let (rows, last) = text.trim_end().rsplit_once('\n').unwrap();
        assert_rows(&frame, 8..=10, &format!("{rows}\n"), depth);
        assert_eq!(crate::testing::bare(last), ",resume N continues one here");
    }
}

#[test]
fn an_ascii_locale_marks_home_with_a_tilde() {
    let options = RenderOptions::new(100).with_terminal(false).with_glyphs(&Glyphs::ASCII);
    let text = list(&scope_rows(), now(), true, &options);
    assert!(text.contains("  2  nftables rules                ~ home   2 h ago "), "{text}");
}

#[test]
fn the_held_question_names_the_terminal() {
    assert_eq!(
        held_question(Some("why is the build slow"), "/dev/pts/4"),
        "\"why is the build slow\" is open in /dev/pts/4. Move it here? [y/N]"
    );
}
