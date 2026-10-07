use efr_protocol::{
    ChangeKind, ClientFrame, ConversationDiff, ConversationDiffResult, ConversationStatus,
    ConversationSummary, ConversationsListResult, ErrorBody, ErrorCode, FileChange, FileChanges,
    Method, Seq,
};
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;

use super::shown;
use crate::error::Exit;
use crate::run;
use crate::testing::{
    TestEnv, capture, command, conversation, now, readable, terminal_facts, turn,
};

const DIFF: &str = "diff --git a/src/main.rs b/src/main.rs\nindex 1111111..2222222 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-\tprintln!(\"old\");\n+\tprintln!(\"a new greeting that is much wider than forty columns\");\n }\ndiff --git a/notes.md b/notes.md\nnew file mode 100644\n--- /dev/null\n+++ b/notes.md\n@@ -0,0 +1 @@\n+# Notes\n";

fn changes() -> FileChanges {
    FileChanges {
        files: vec![
            FileChange {
                path: "src/main.rs".to_owned(),
                kind: ChangeKind::Modified,
                from: None,
                added: 1,
                removed: 1,
                binary: false,
            },
            FileChange {
                path: "notes.md".to_owned(),
                kind: ChangeKind::Added,
                from: None,
                added: 1,
                removed: 0,
                binary: false,
            },
        ],
        more: 0,
        added: 2,
        removed: 1,
    }
}

fn result(diff: Option<&str>) -> ConversationDiffResult {
    ConversationDiffResult { turn_id: turn(), changes: changes(), diff: diff.map(str::to_owned) }
}

fn summary() -> ConversationSummary {
    ConversationSummary {
        id: conversation(),
        title: Some("greet".to_owned()),
        status: ConversationStatus::Idle,
        created_at: now(),
        updated_at: now(),
        last_seq: Seq::new(9),
        cwd: None,
        scope: None,
        tty: None,
    }
}

#[test]
fn a_diff_on_a_terminal_is_painted_and_ends_with_its_counts() {
    let mut out = Vec::new();
    for cols in [40_u16, 80] {
        for (name, colour) in [("colour", ColourMode::Ansi16), ("NO_COLOR", ColourMode::None)] {
            let options = RenderOptions::new(cols).with_colour(colour);
            let text = shown(&result(Some(DIFF)), false, &options);
            out.push(format!("=== {cols} columns, {name}\n{}", readable(&text)));
        }
    }
    insta::assert_snapshot!(out.join("\n"));
}

#[test]
fn in_a_pipe_the_diff_is_the_daemons_text() {
    let options = RenderOptions::new(80).with_terminal(false);
    assert_eq!(shown(&result(Some(DIFF)), false, &options), DIFF);
    // A control character cannot reach a terminal behind the pipe; a tab stays.
    let odd = "@@ -1 +1 @@\n-\told\n+new\x1b[2J\n";
    assert_eq!(
        shown(&result(Some(odd)), false, &options),
        "@@ -1 +1 @@\n-\told\n+new\u{241b}[2J\n"
    );
}

#[test]
fn a_cut_diff_counts_the_lines_it_left_out() {
    let options = RenderOptions::new(80).with_colour(ColourMode::None);
    let cut = "@@ -1 +1,3 @@\n-old\n+new\n+more\n... 19997 more lines\n";
    let text = readable(&shown(&result(Some(cut)), false, &options));
    assert!(text.contains("\u{2026} 19997 more lines"), "{text}");
    assert!(!text.contains("... 19997"), "{text}");
}

#[test]
fn the_stat_lists_the_files() {
    let options = RenderOptions::new(80).with_terminal(false);
    assert_eq!(
        shown(&result(None), true, &options),
        "changed src/main.rs  +1 \u{2212}1\nnew     notes.md     +1\n2 files changed, +2 \u{2212}1\n"
    );
}

/// Runs `efr <args>` against a daemon that lists the one conversation and answers
/// `conversation.diff` with `answer`; returns the exit, stdout, stderr and the params.
async fn diff_with(
    args: &[&str],
    terminal: bool,
    answer: Result<ConversationDiffResult, ErrorBody>,
    listed: Vec<ConversationSummary>,
) -> (Exit, String, String, Option<ConversationDiff>) {
    let env = TestEnv::new();
    let daemon = env.listen();
    let mut ctx = env.context();
    if terminal {
        ctx.term = terminal_facts();
    }
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let mut asked = None;
        while let Some(frame) = conn.recv().await {
            let ClientFrame::Request { id, method } = frame else { continue };
            match method {
                Method::ConversationsList(_) => {
                    let list = ConversationsListResult {
                        conversations: listed.clone(),
                        next_cursor: None,
                    };
                    conn.reply(id, &list).await;
                }
                Method::ConversationDiff(params) => {
                    asked = Some(params);
                    match &answer {
                        Ok(result) => conn.reply(id, result).await,
                        Err(body) => conn.fail(id, body.clone()).await,
                    }
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        asked
    };
    let mut line = vec!["diff"];
    line.extend_from_slice(args);
    let line = command(&line);
    let (exit, asked) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    (exit, captured.stdout(), captured.stderr(), asked)
}

#[tokio::test]
async fn efr_diff_shows_the_last_turn_of_the_newest_conversation() {
    let (exit, stdout, stderr, asked) =
        diff_with(&[], false, Ok(result(Some(DIFF))), vec![summary()]).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(
        asked,
        Some(ConversationDiff {
            conversation_id: Some(conversation()),
            turn_id: None,
            stat: false
        })
    );
    assert_eq!(stdout, DIFF);
    assert_eq!(
        stderr,
        "the last turn of the newest conversation; efr diff --turn <id> shows another\n"
    );
}

#[tokio::test]
async fn efr_diff_stat_on_a_terminal_paints_the_counts() {
    let (exit, stdout, _, _) =
        diff_with(&["--stat"], true, Ok(result(None)), vec![summary()]).await;
    assert_eq!(exit, Exit::Success);
    insta::assert_snapshot!(readable(&stdout));
}

#[tokio::test]
async fn efr_diff_turn_asks_for_that_turn_without_a_lookup() {
    let turn = turn().to_string();
    let (exit, _, stderr, asked) =
        diff_with(&["--turn", &turn, "--stat"], false, Ok(result(None)), Vec::new()).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(
        asked,
        Some(ConversationDiff {
            conversation_id: None,
            turn_id: Some(crate::testing::turn()),
            stat: true
        })
    );
    assert_eq!(stderr, "");
}

#[tokio::test]
async fn a_turn_that_changed_nothing_says_so() {
    let nothing = ConversationDiffResult {
        turn_id: turn(),
        changes: FileChanges::default(),
        diff: Some(String::new()),
    };
    let (exit, stdout, stderr, _) = diff_with(&[], false, Ok(nothing), vec![summary()]).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(stdout, "");
    assert!(stderr.ends_with(&format!("turn {} changed no files\n", turn())), "{stderr}");
}

#[tokio::test]
async fn a_turn_without_a_snapshot_is_an_error_that_says_which_turn() {
    let body = ErrorBody::new(ErrorCode::NotFound, "the conversation has no turn");
    let (exit, stdout, stderr, _) = diff_with(&[], false, Err(body), vec![summary()]).await;
    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(stdout, "");
    assert!(
        stderr.ends_with(&format!(
            "efr: there is no diff of the last turn of conversation {}: the conversation has no turn\n",
            conversation()
        )),
        "{stderr}"
    );
}

#[tokio::test]
async fn without_a_conversation_there_is_nothing_to_show() {
    let (exit, stdout, stderr, asked) = diff_with(&[], false, Ok(result(None)), Vec::new()).await;
    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(stdout, "");
    assert_eq!(asked, None);
    assert_eq!(stderr, "efr: there is no conversation yet, so no turn changed a file\n");
}
