use std::path::PathBuf;
use std::sync::Arc;

use efr_protocol::{
    ClientFrame, ConversationDelete, ConversationDeleteResult, ConversationId, ConversationStatus,
    ConversationSummary, ConversationsListResult, ErrorBody, ErrorCode, Method, Seq,
};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::context::Context;
use crate::error::Exit;
use crate::keys::Keys;
use crate::run;
use crate::testing::{ScriptedKeys, TestEnv, bare, capture, command, now};

const TTY: &str = "/dev/pts/7";
const ID: &str = "019a9b1c-3d00-7a10-8b20-000000000002";

fn id() -> ConversationId {
    ID.parse().unwrap()
}

fn listed() -> Vec<ConversationSummary> {
    vec![ConversationSummary {
        id: id(),
        title: Some("why is the build slow".to_owned()),
        status: ConversationStatus::Idle,
        created_at: now(),
        updated_at: now(),
        last_seq: Seq::new(2),
        cwd: None,
        scope: None,
        tty: None,
    }]
}

fn deleted(jobs: u32, not_removed: Vec<PathBuf>) -> ConversationDeleteResult {
    ConversationDeleteResult::Deleted {
        conversation_id: id(),
        title: Some("why is the build slow".to_owned()),
        jobs_stopped: jobs,
        not_removed,
    }
}

fn held() -> ConversationDeleteResult {
    ConversationDeleteResult::Held {
        tty: "/dev/pts/4".to_owned(),
        title: Some("why is the build slow".to_owned()),
    }
}

fn busy(what: &str) -> ErrorBody {
    ErrorBody::new(ErrorCode::Busy, "a turn runs").with_data(json!({ "busy": what }))
}

/// What efr does on the terminal: its keys come from the test, or there are none.
enum Terminal {
    Keys { keys: Arc<ScriptedKeys>, presses: Vec<u8> },
    Pipe,
}

/// What a run of `efr conversation delete <args>` did.
struct Ran {
    exit: Exit,
    stdout: String,
    stderr: String,
    deletes: Vec<ConversationDelete>,
}

/// A context whose plugin handed over the shell context of [`TTY`].
fn context(env: &TestEnv, keys: Option<Arc<ScriptedKeys>>) -> Context {
    let shell = format!(r#"{{"pwd":"/srv","tty":"{TTY}","shell_pid":4242}}"#);
    let ctx = Context { env: Env::fixed(vec![(Var::Context, shell)]), ..env.context() };
    match keys {
        Some(keys) => Context { keys: keys as Arc<dyn Keys>, ..ctx },
        None => ctx,
    }
}

/// Runs `efr conversation delete <args>` against a fake daemon that lists [`listed`]
/// and answers each `conversation.delete` with the next of `answers`. On a terminal the
/// test presses the next of its keys after the list and after each answer that is not
/// a delete.
async fn delete_with(
    args: &[&str],
    terminal: Terminal,
    answers: Vec<Result<ConversationDeleteResult, ErrorBody>>,
) -> Ran {
    let env = TestEnv::new();
    let fake = env.listen();
    let (keys, mut presses) = match terminal {
        Terminal::Keys { keys, presses } => (Some(keys), presses.into_iter()),
        Terminal::Pipe => (None, Vec::new().into_iter()),
    };
    let ctx = context(&env, keys.clone());
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = fake.accept().await;
        let mut deletes = Vec::new();
        let mut answers = answers.into_iter();
        while let Some(frame) = conn.recv().await {
            let ClientFrame::Request { id: request, method } = frame else { continue };
            let press = match method {
                Method::ConversationsList(_) => {
                    let list =
                        ConversationsListResult { conversations: listed(), next_cursor: None };
                    conn.reply(request, &list).await;
                    true
                }
                Method::ConversationDelete(params) => {
                    deletes.push(params);
                    match answers.next().expect("an answer for each delete") {
                        Ok(result) => {
                            let done = matches!(result, ConversationDeleteResult::Deleted { .. });
                            conn.reply(request, &result).await;
                            !done
                        }
                        Err(body) => {
                            conn.fail(request, body).await;
                            true
                        }
                    }
                }
                other => panic!("unexpected {}", other.name()),
            };
            if press && let (Some(keys), Some(key)) = (&keys, presses.next()) {
                keys.press(key).await;
            }
        }
        deletes
    };
    let mut line = vec!["conversation", "delete"];
    line.extend_from_slice(args);
    let line = command(&line);
    let (exit, deletes) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    Ran { exit, stdout: bare(&captured.stdout()), stderr: bare(&captured.stderr()), deletes }
}

fn on_a_terminal(presses: &[u8]) -> Terminal {
    Terminal::Keys { keys: Arc::new(ScriptedKeys::default()), presses: presses.to_vec() }
}

#[tokio::test]
async fn delete_asks_first_and_y_deletes_the_conversation_of_the_id_start() {
    let ran = Box::pin(delete_with(
        &["019a9b1c-3d00"],
        on_a_terminal(b"y"),
        vec![Ok(deleted(1, Vec::new()))],
    ))
    .await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(
        ran.stdout,
        "Delete \"why is the build slow\"? Nothing takes a delete back. [y/N]\n\
         deleted \"why is the build slow\"; efrd stopped its background job\n"
    );
    let [delete] = ran.deletes.try_into().unwrap();
    assert_eq!((delete.conversation_id, delete.force, delete.stop), (id(), false, false));
}

#[tokio::test]
async fn delete_n_deletes_nothing() {
    let ran = Box::pin(delete_with(&[ID], on_a_terminal(b"n"), Vec::new())).await;

    assert_eq!(ran.exit, Exit::Success);
    assert!(ran.deletes.is_empty(), "n sends no delete");
    assert_eq!(ran.stderr, "nothing was deleted\n");
}

#[tokio::test]
async fn delete_without_a_terminal_needs_yes() {
    let env = TestEnv::new();
    let ctx = context(&env, None);
    let (mut out, captured) = capture();
    let line = command(&["conversation", "delete", ID]);

    let exit = run::run(&line, &ctx, &mut out).await;

    assert_eq!(exit, Exit::Usage);
    assert_eq!(
        captured.stderr(),
        "efr: efr conversation delete needs a terminal or --yes; nothing was deleted\n"
    );
}

#[tokio::test]
async fn yes_deletes_with_no_question_and_names_what_stayed() {
    let left = PathBuf::from("/d/scratch/x");
    let ran =
        Box::pin(delete_with(&["--yes", ID], Terminal::Pipe, vec![Ok(deleted(0, vec![left]))]))
            .await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(ran.stdout, "deleted \"why is the build slow\"\n");
    assert_eq!(
        ran.stderr,
        "efrd could not remove /d/scratch/x; it tries again at its next start\n"
    );
}

#[tokio::test]
async fn a_held_conversation_asks_and_y_deletes_it_with_force() {
    let ran = Box::pin(delete_with(
        &[ID],
        on_a_terminal(b"yy"),
        vec![Ok(held()), Ok(deleted(0, Vec::new()))],
    ))
    .await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert!(
        ran.stdout.contains(
            "\"why is the build slow\" is open in /dev/pts/4. Delete it there too? [y/N]\n"
        ),
        "{}",
        ran.stdout
    );
    let [first, second] = ran.deletes.try_into().unwrap();
    assert!(!first.force && second.force && !second.stop);
    assert_ne!(first.command_id, second.command_id, "a new command, so no receipt answers it");
}

#[tokio::test]
async fn a_held_conversation_and_n_deletes_nothing() {
    let ran = Box::pin(delete_with(&[ID], on_a_terminal(b"yn"), vec![Ok(held())])).await;

    assert_eq!(ran.exit, Exit::Success);
    assert_eq!(ran.deletes.len(), 1);
    assert_eq!(ran.stderr, "nothing was deleted\n");
}

#[tokio::test]
async fn a_held_conversation_in_a_pipe_needs_force() {
    let ran = Box::pin(delete_with(&["--yes", ID], Terminal::Pipe, vec![Ok(held())])).await;

    assert_eq!(ran.exit, Exit::DaemonError);
    assert!(
        ran.stderr.contains(&format!(
            "\"why is the build slow\" is open in /dev/pts/4; efr conversation delete --force {ID} deletes it"
        )),
        "{}",
        ran.stderr
    );
}

#[tokio::test]
async fn a_running_turn_asks_and_y_stops_it_first() {
    let ran = Box::pin(delete_with(
        &[ID],
        on_a_terminal(b"yy"),
        vec![Err(busy("turn")), Ok(deleted(0, Vec::new()))],
    ))
    .await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert!(
        ran.stdout
            .contains("A turn runs in \"why is the build slow\". Stop it and delete? [y/N]\n"),
        "{}",
        ran.stdout
    );
    let [first, second] = ran.deletes.try_into().unwrap();
    assert!(!first.stop && second.stop && !second.force);
}

#[tokio::test]
async fn a_running_undo_is_an_error_with_no_question() {
    let ran = Box::pin(delete_with(&[ID], on_a_terminal(b"y"), vec![Err(busy("undo"))])).await;

    assert_eq!(ran.exit, Exit::DaemonError);
    assert_eq!(ran.deletes.len(), 1);
    assert!(!ran.stdout.contains("Stop it"), "{}", ran.stdout);
}

#[tokio::test]
async fn a_running_turn_in_a_pipe_needs_force_and_force_sends_both() {
    let ran = Box::pin(delete_with(&["--yes", ID], Terminal::Pipe, vec![Err(busy("turn"))])).await;
    assert_eq!(ran.exit, Exit::DaemonError);
    assert!(
        ran.stderr.contains(&format!("efr conversation delete --force {ID} stops it")),
        "{}",
        ran.stderr
    );

    let ran = Box::pin(delete_with(
        &["--yes", "--force", ID],
        Terminal::Pipe,
        vec![Ok(deleted(0, Vec::new()))],
    ))
    .await;
    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let [delete] = ran.deletes.try_into().unwrap();
    assert!(delete.force && delete.stop);
}

#[tokio::test]
async fn an_older_efrd_says_restart() {
    let unknown = ErrorBody::new(ErrorCode::Invalid, "the frame is not a valid client frame");
    let ran = Box::pin(delete_with(&["--yes", ID], Terminal::Pipe, vec![Err(unknown)])).await;

    assert_eq!(ran.exit, Exit::DaemonError);
    assert!(ran.stderr.contains("efrd is older than this efr"), "{}", ran.stderr);
}
