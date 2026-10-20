use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    ClientFrame, ConversationStatus, ConversationSummary, ConversationsListResult, ErrorBody,
    ErrorCode, Method, RefChange, Seq, TurnUndo, TurnUndoResult, UndoAction, UndoEntry, UndoReason,
    UndoableTurn,
};
use efr_render::{ColourMode, RenderOptions};
use pretty_assertions::assert_eq;

use super::{preview_rows, report};
use crate::context::Context;
use crate::error::Exit;
use crate::keys::Keys;
use crate::run;
use crate::testing::{ScriptedKeys, TestEnv, capture, command, conversation, now, readable, turn};

fn entry(action: UndoAction, path: &str, reason: Option<UndoReason>) -> UndoEntry {
    UndoEntry { action, path: path.to_owned(), reason }
}

/// The preview of an undo of [`turn`]: one path of each kind, a ref and a note.
fn preview() -> TurnUndoResult {
    TurnUndoResult {
        conversation_id: conversation(),
        turn_id: Some(turn()),
        title: Some("change a.txt\nand add notes".to_owned()),
        undo_id: None,
        applied: false,
        stopped: false,
        resumed: false,
        entries: vec![
            entry(UndoAction::Restore, "a.txt", None),
            entry(UndoAction::Trash, "notes.md", None),
            entry(
                UndoAction::Delete,
                "~/.config/systemd/user/default.target.wants/x.service",
                None,
            ),
            entry(UndoAction::Command, "@crontab", None),
            entry(UndoAction::Skip, "b.txt", Some(UndoReason::ChangedAfterTurn)),
            entry(UndoAction::NotCovered, ".env.large", Some(UndoReason::LeftOut)),
        ],
        entries_more: 2,
        refs: vec![RefChange {
            root: "efr-undo".to_owned(),
            name: "main".to_owned(),
            from: Some("1a2b3c4".to_owned()),
            to: Some("5d6e7f8".to_owned()),
            applied: None,
            reason: None,
        }],
        trash: None,
        notes: vec!["your git index is unchanged; git status can show staged changes".to_owned()],
        turns: Vec::new(),
    }
}

/// What the daemon answers when it applied [`preview`].
fn applied() -> TurnUndoResult {
    let mut result = preview();
    result.applied = true;
    result.refs[0].applied = Some(true);
    result.trash = Some(PathBuf::from("/home/user/.local/share/efr/undo-trash/u-0193"));
    result
}

fn summary() -> ConversationSummary {
    ConversationSummary {
        id: conversation(),
        title: Some("change a.txt".to_owned()),
        status: ConversationStatus::Idle,
        created_at: now(),
        updated_at: now(),
        last_seq: Seq::new(9),
        cwd: None,
        scope: None,
        tty: None,
    }
}

fn plain() -> RenderOptions {
    RenderOptions::new(80).with_terminal(false)
}

#[test]
fn efr_undo_preview_rows() {
    assert_eq!(
        preview_rows(&preview(), false, &plain()),
        format!(
            "undo turn {}: change a.txt\n\
             restore     a.txt\n\
             trash       notes.md\n\
             delete      ~/.config/systemd/user/default.target.wants/x.service\n\
             command     @crontab\n\
             skip        b.txt (changed after the turn; --force overwrites it)\n\
             not covered .env.large (the snapshot left it out)\n\
             \u{2026} 2 more\n\
             ref         main 1a2b3c4 -> 5d6e7f8\n\
             your git index is unchanged; git status can show staged changes\n",
            turn()
        )
    );
    let painted =
        preview_rows(&preview(), true, &RenderOptions::new(80).with_colour(ColourMode::Ansi16));
    insta::assert_snapshot!(readable(&painted));
}

#[test]
fn the_report_counts_what_the_undo_did_and_names_the_trash() {
    let home = Path::new("/home/user");
    assert_eq!(
        report(&applied(), false, Some(home), &plain()),
        "undone: 2 files restored, 1 file moved to ~/.local/share/efr/undo-trash/u-0193/, \
         1 link removed, 1 ref moved, 2 left as they are\n\
         your git index is unchanged; git status can show staged changes\n"
    );
    let nothing = TurnUndoResult { applied: false, notes: Vec::new(), ..applied() };
    assert_eq!(report(&nothing, true, Some(home), &plain()), "redone: nothing changed\n");
}

/// Runs `efr undo <args>` against a daemon that lists the one conversation and answers
/// each `turn.undo` with the next of `answers`; returns the exit, stdout, stderr and
/// every `turn.undo` params.
async fn undo_with(
    args: &[&str],
    keys: Option<Arc<ScriptedKeys>>,
    answers: Vec<Result<TurnUndoResult, ErrorBody>>,
) -> (Exit, String, String, Vec<TurnUndo>) {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = match &keys {
        Some(keys) => Context { keys: Arc::clone(keys) as Arc<dyn Keys>, ..env.context() },
        None => env.context(),
    };
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let mut asked = Vec::new();
        let mut answers = answers.into_iter();
        while let Some(frame) = conn.recv().await {
            let ClientFrame::Request { id, method } = frame else { continue };
            match method {
                Method::ConversationsList(_) => {
                    let list = ConversationsListResult {
                        conversations: vec![summary()],
                        next_cursor: None,
                    };
                    conn.reply(id, &list).await;
                }
                Method::TurnUndo(params) => {
                    asked.push(params);
                    match answers.next().expect("an answer for each turn.undo") {
                        Ok(result) => conn.reply(id, &result).await,
                        Err(body) => conn.fail(id, body).await,
                    }
                    if let Some(keys) = &keys
                        && asked.len() == 1
                    {
                        keys.press(b'x').await;
                        keys.press(b'y').await;
                    }
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        asked
    };
    let mut line = vec!["undo"];
    line.extend_from_slice(args);
    let line = command(&line);
    let (exit, asked) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    (exit, captured.stdout(), captured.stderr(), asked)
}

#[tokio::test]
async fn efr_undo_needs_yes_without_a_terminal() {
    let (exit, stdout, stderr, asked) = undo_with(&[], None, vec![Ok(preview())]).await;

    assert_eq!(exit, Exit::Usage);
    assert_eq!(stdout, preview_rows(&preview(), false, &plain()));
    assert!(
        stderr.ends_with("efr: efr undo needs a terminal or --yes; nothing changed\n"),
        "{stderr}"
    );
    let [dry_run] = asked.try_into().expect("only the preview");
    assert!(dry_run.dry_run);
    assert_eq!(dry_run.conversation_id, Some(conversation()));
    assert_eq!(dry_run.turn_id, None);
}

#[tokio::test]
async fn efr_undo_yes_applies_the_turn_of_the_preview() {
    let (exit, stdout, _, asked) =
        undo_with(&["--yes", "--force"], None, vec![Ok(preview()), Ok(applied())]).await;

    assert_eq!(exit, Exit::Success);
    let [dry_run, apply] = asked.try_into().expect("a preview, then the undo");
    assert!(dry_run.dry_run && dry_run.force);
    assert!(!apply.dry_run && apply.force);
    assert_eq!(apply.turn_id, Some(turn()), "the turn of the preview");
    assert_ne!(apply.command_id, dry_run.command_id);
    assert!(stdout.contains("\nundone: 2 files restored, 1 file moved to "), "{stdout}");
}

#[tokio::test]
async fn a_key_answers_the_question() {
    let keys = Arc::new(ScriptedKeys::default());
    let (exit, stdout, _, asked) =
        undo_with(&[], Some(keys), vec![Ok(preview()), Ok(applied())]).await;

    assert_eq!(exit, Exit::Success);
    assert_eq!(asked.len(), 2, "y applies it");
    assert!(stdout.contains("undo? y = yes, n = no\n"), "{stdout}");
    assert!(stdout.contains("undone: "), "{stdout}");
}

#[tokio::test]
async fn efr_undo_dry_run_changes_nothing() {
    let (exit, stdout, _, asked) = undo_with(&["--dry-run"], None, vec![Ok(preview())]).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(asked.len(), 1);
    assert!(stdout.starts_with("undo turn "), "{stdout}");
}

#[tokio::test]
async fn efr_undo_list_shows_undone_turns() {
    let listed = TurnUndoResult {
        entries: Vec::new(),
        refs: Vec::new(),
        notes: Vec::new(),
        turn_id: None,
        turns: vec![
            UndoableTurn {
                turn_id: turn(),
                title: Some("change a.txt".to_owned()),
                auto: true,
                full: false,
                undone: true,
                stopped: false,
                ended: Some(now()),
            },
            UndoableTurn {
                turn_id: turn(),
                title: Some("clean up".to_owned()),
                auto: true,
                full: true,
                undone: false,
                stopped: false,
                ended: Some(now()),
            },
            UndoableTurn {
                turn_id: turn(),
                title: Some("build it".to_owned()),
                auto: false,
                full: false,
                undone: false,
                stopped: true,
                ended: None,
            },
        ],
        ..preview()
    };
    let (exit, stdout, _, asked) = undo_with(&["--list"], None, vec![Ok(listed)]).await;

    assert_eq!(exit, Exit::Success);
    let [list] = asked.try_into().expect("one list");
    assert!(list.list && !list.dry_run);
    assert_eq!(
        stdout,
        format!(
            "{turn}  undone   auto  change a.txt\n{turn}           full  clean up\n\
             {turn}  stopped        build it\n",
            turn = turn()
        )
    );
}

#[tokio::test]
async fn a_refused_undo_says_why_and_what_to_do() {
    let body =
        ErrorBody::new(ErrorCode::Conflict, format!("the later turn {} is not undone", turn()))
            .with_data(serde_json::json!({
                "refused": { "kind": "later_turns_not_undone", "turn_id": turn() }
            }));
    let (exit, stdout, stderr, _) = undo_with(&[], None, vec![Err(body)]).await;

    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(stdout, "");
    assert!(
        stderr.ends_with(&format!(
            "efr: efr undo changed nothing: the later turn {} is not undone\n\
             efr: undo the newer turn first, or add --force to undo this one anyway\n",
            turn()
        )),
        "{stderr}"
    );
}

#[tokio::test]
async fn efr_undo_without_force_fails_when_only_force_acts() {
    let skipped = TurnUndoResult {
        entries: vec![
            entry(UndoAction::Skip, "job.txt", Some(UndoReason::ChangedAfterTurn)),
            entry(UndoAction::NotCovered, ".env.large", Some(UndoReason::LeftOut)),
        ],
        refs: Vec::new(),
        ..preview()
    };
    let (exit, stdout, stderr, asked) =
        undo_with(&["--yes"], None, vec![Ok(skipped.clone())]).await;

    assert_eq!(exit, Exit::DaemonError);
    assert_eq!(stdout, preview_rows(&skipped, false, &plain()));
    assert!(
        stderr.ends_with(
            "efr: efr undo changed nothing: 1 path acts only with --force; efr undo --force acts on it\n"
        ),
        "{stderr}"
    );
    assert_eq!(asked.len(), 1, "only the preview");

    let (exit, _, _, asked) =
        undo_with(&["--yes", "--force"], None, vec![Ok(skipped), Ok(applied())]).await;
    assert_eq!(exit, Exit::Success);
    assert_eq!(asked.len(), 2, "--force applies it");
}
