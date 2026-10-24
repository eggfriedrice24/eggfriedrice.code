use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    Capabilities, ClientFrame, ConversationDelete, ConversationDeleteResult, ConversationHistory,
    ConversationHistoryResult, ConversationId, ConversationResume, ConversationResumeResult,
    ConversationStatus, ConversationSummary, ConversationsListResult, ConversationsRecent,
    ConversationsRecentResult, ConversationsSearch, ConversationsSearchResult, ErrorBody,
    ErrorCode, Method, RecentConversation, RouteLabel, RouteScope, SearchRow, Seq,
};
use efr_stdx::env::{Env, Var};
use pretty_assertions::assert_eq;

use super::Target;
use crate::context::{Context, Interrupt, Terminate};
use crate::error::Exit;
use crate::format::resume;
use crate::keys::Keys;
use crate::live::effective_width;
use crate::picker::{NoPicker, Outcome, Picker, Version, fzf_args};
use crate::run;
use crate::settings::Settings;
use crate::terminal::TermFacts;
use crate::testing::{
    Script, ScriptedKeys, ScriptedPicker, TestEnv, TestInterrupt, TestTerminate, bare, capture,
    command, hello_result, now, terminal_facts,
};

const TTY: &str = "/dev/pts/7";
const CONVERSATION_ID: &str = "019a9b1c-3d00-7a10-8b20-000000000002";

fn id(n: u64) -> ConversationId {
    format!("019a9b1c-3d00-7a10-8b20-{n:012x}").parse().unwrap()
}

fn summary(n: u64, title: &str) -> ConversationSummary {
    ConversationSummary {
        id: id(n),
        title: Some(title.to_owned()),
        status: ConversationStatus::Idle,
        created_at: now(),
        updated_at: now(),
        last_seq: Seq::new(n),
        cwd: None,
        scope: None,
        tty: None,
    }
}

fn recent() -> ConversationsRecentResult {
    let row = |n, title| RecentConversation {
        summary: summary(n, title),
        route: RouteScope::Outside,
        project: None,
        turns: 1,
        open_in: None,
        here: false,
    };
    ConversationsRecentResult {
        scope: None,
        current: None,
        conversations: vec![row(1, "fix the tests"), row(2, "why is the build slow")],
    }
}

fn resumed(n: u64) -> ConversationResumeResult {
    ConversationResumeResult::Resumed {
        conversation_id: id(n),
        scope: RouteLabel { scope: RouteScope::Outside, name: None, dir: PathBuf::from("/srv") },
        title: Some("why is the build slow".to_owned()),
        updated_at: now(),
        shell_dir: None,
        here: false,
        continues_here: true,
    }
}

fn held() -> ConversationResumeResult {
    ConversationResumeResult::Held {
        tty: "/dev/pts/4".to_owned(),
        title: Some("why is the build slow".to_owned()),
    }
}

/// A context of a terminal whose plugin handed over its shell context.
fn terminal(env: &TestEnv) -> Context {
    let context = format!(r#"{{"pwd":"/srv","tty":"{TTY}","shell_pid":4242}}"#);
    Context { env: Env::fixed(vec![(Var::Context, context)]), ..env.context() }
}

/// What a run of `efr resume <args>` did.
struct Ran {
    exit: Exit,
    stdout: String,
    stderr: String,
    recent: Vec<ConversationsRecent>,
    resumes: Vec<ConversationResume>,
    searches: Vec<ConversationsSearch>,
    histories: Vec<ConversationHistory>,
    deletes: Vec<ConversationDelete>,
}

/// What the fake daemon answers.
#[derive(Default)]
struct Daemon {
    /// For `conversations.list`.
    listed: Vec<ConversationSummary>,
    /// For each `conversation.resume`, in turn.
    answers: Vec<Result<ConversationResumeResult, ErrorBody>>,
    /// The key that the test presses after each `held` answer.
    keys: Option<(Arc<ScriptedKeys>, u8)>,
    /// hello announces the search methods.
    searching: bool,
    /// For each `conversation.delete`, in turn.
    deletes: Vec<Result<ConversationDeleteResult, ErrorBody>>,
}

/// Runs `efr resume <args>` against a daemon that lists [`recent`], lists `listed` for
/// `conversations.list`, and answers each `conversation.resume` with the next of
/// `answers`. With `keys`, the test presses `key` after each `held` answer.
async fn resume_with(
    ctx: Context,
    env: &TestEnv,
    args: &[&str],
    listed: Vec<ConversationSummary>,
    answers: Vec<Result<ConversationResumeResult, ErrorBody>>,
    keys: Option<(Arc<ScriptedKeys>, u8)>,
) -> Ran {
    resume_in(ctx, env, args, Daemon { listed, answers, keys, ..Daemon::default() }).await
}

/// Runs `efr resume <args>` against the fake `daemon`. A search answers
/// [`search_result`], the history of a conversation an empty page.
async fn resume_in(ctx: Context, env: &TestEnv, args: &[&str], daemon: Daemon) -> Ran {
    let Daemon { listed, answers, keys, searching, deletes } = daemon;
    let fake = env.listen();
    let ctx = match &keys {
        Some((keys, _)) => Context { keys: Arc::clone(keys) as Arc<dyn Keys>, ..ctx },
        None => ctx,
    };
    let (mut out, captured) = capture();
    let script = async {
        let capabilities =
            Capabilities { search: searching.then_some(true), ..hello_result().capabilities };
        let mut conn = fake.accept_with(capabilities).await;
        let mut seen = Seen::default();
        let mut answers = answers.into_iter();
        let mut deletes = deletes.into_iter();
        while let Some(frame) = conn.recv().await {
            let ClientFrame::Request { id, method } = frame else { continue };
            match method {
                Method::ConversationsRecent(params) => {
                    seen.recent.push(params);
                    conn.reply(id, &recent()).await;
                }
                Method::ConversationsSearch(params) => {
                    seen.searches.push(params);
                    conn.reply(id, &search_result()).await;
                }
                Method::ConversationHistory(params) => {
                    seen.histories.push(params);
                    let page = ConversationHistoryResult { events: Vec::new(), next_cursor: None };
                    conn.reply(id, &page).await;
                }
                Method::ConversationsList(_) => {
                    let list = ConversationsListResult {
                        conversations: listed.clone(),
                        next_cursor: None,
                    };
                    conn.reply(id, &list).await;
                }
                Method::ConversationDelete(params) => {
                    seen.deletes.push(params);
                    match deletes.next().expect("an answer for each delete") {
                        Ok(result) => conn.reply(id, &result).await,
                        Err(body) => conn.fail(id, body).await,
                    }
                }
                Method::ConversationResume(params) => {
                    seen.resumes.push(params);
                    match answers.next().expect("an answer for each resume") {
                        Ok(result) => {
                            let is_held = matches!(result, ConversationResumeResult::Held { .. });
                            conn.reply(id, &result).await;
                            if let (true, Some((keys, key))) = (is_held, &keys) {
                                keys.press(*key).await;
                            }
                        }
                        Err(body) => conn.fail(id, body).await,
                    }
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
        seen
    };
    let mut line = vec!["resume"];
    line.extend_from_slice(args);
    let line = command(&line);
    let (exit, seen) = tokio::join!(run::run(&line, &ctx, &mut out), script);
    Ran {
        exit,
        stdout: captured.stdout(),
        stderr: captured.stderr(),
        recent: seen.recent,
        resumes: seen.resumes,
        searches: seen.searches,
        histories: seen.histories,
        deletes: seen.deletes,
    }
}

/// The requests that the fake daemon got.
#[derive(Default)]
struct Seen {
    recent: Vec<ConversationsRecent>,
    resumes: Vec<ConversationResume>,
    searches: Vec<ConversationsSearch>,
    histories: Vec<ConversationHistory>,
    deletes: Vec<ConversationDelete>,
}

/// What a search answers: the rows of [`recent`], with their whole ids.
fn search_result() -> ConversationsSearchResult {
    let rows = recent()
        .conversations
        .into_iter()
        .map(|conversation| SearchRow {
            short_id: conversation.summary.id.to_string(),
            conversation,
            title_ranges: Vec::new(),
            hit: None,
        })
        .collect();
    ConversationsSearchResult { rows, ..ConversationsSearchResult::default() }
}

#[test]
fn resume_two_digits_is_n_hex_is_an_id_start_and_the_rest_a_search() {
    let search = |words: &str| Target::Search(words.to_owned());
    assert_eq!(Target::of("2"), Target::Row(2));
    assert_eq!(Target::of("12"), Target::Row(12));
    assert_eq!(Target::of(" 7 "), Target::Row(7));
    assert_eq!(Target::of("019a"), Target::Id("019a".to_owned()));
    assert_eq!(Target::of("019A9B1C-3d00"), Target::Id("019A9B1C-3d00".to_owned()));
    assert_eq!(Target::of(CONVERSATION_ID), Target::Id(CONVERSATION_ID.to_owned()));
    assert_eq!(Target::of("cafe"), Target::Id("cafe".to_owned()), "a hex word starts an id first");
    assert_eq!(Target::of("123"), search("123"), "3 characters start no id");
    assert_eq!(Target::of("+1"), search("+1"));
    assert_eq!(Target::of("rsync"), search("rsync"));
    assert_eq!(Target::of("019a 2"), search("019a 2"), "two words are a search");
    assert_eq!(Target::of("fix the tests"), search("fix the tests"));
}

#[tokio::test]
async fn resume_n_resumes_row_n_of_the_list_that_efrd_computes() {
    let env = TestEnv::new();
    let ran =
        Box::pin(resume_with(terminal(&env), &env, &["2"], Vec::new(), vec![Ok(resumed(2))], None))
            .await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let [asked] = ran.recent.try_into().unwrap();
    assert_eq!((asked.tty.as_deref(), asked.pwd), (Some(TTY), Some(PathBuf::from("/srv"))));
    let [resume] = ran.resumes.try_into().unwrap();
    assert_eq!((resume.conversation_id, resume.take), (id(2), false));
    assert_eq!(resume.context.shell_pid, Some(4242));
    assert_eq!(ran.stdout, "↪ resumed \"why is the build slow\" (no project, just now)\n");
}

#[tokio::test]
async fn resume_lists_with_the_plugins_hint() {
    let env = TestEnv::new();
    let ran = Box::pin(resume_with(terminal(&env), &env, &[], Vec::new(), Vec::new(), None)).await;

    assert_eq!(ran.exit, Exit::Success);
    assert!(ran.stdout.starts_with(" 1  fix the tests"), "{}", ran.stdout);
    assert!(ran.stdout.ends_with(",resume N continues one here\n"), "{}", ran.stdout);
}

#[tokio::test]
async fn resume_of_a_row_that_is_not_there_fails() {
    let env = TestEnv::new();
    let ran =
        Box::pin(resume_with(terminal(&env), &env, &["9"], Vec::new(), Vec::new(), None)).await;

    assert_eq!(ran.exit, Exit::Usage);
    assert!(ran.stderr.contains("has no row 9; it has 2"), "{}", ran.stderr);
}

#[tokio::test]
async fn resume_ambiguous_id_start_fails_with_the_matches() {
    let env = TestEnv::new();
    let listed = vec![summary(1, "fix the tests"), summary(2, "why is the build slow")];
    let ran =
        Box::pin(resume_with(terminal(&env), &env, &["019a9b1c"], listed, Vec::new(), None)).await;

    assert_eq!(ran.exit, Exit::Usage);
    assert!(ran.resumes.is_empty());
    assert!(
        ran.stderr.contains(&format!(
            "\"019a9b1c\" matches 2 conversations: {} (fix the tests), {} (why is the build slow); type more of the id",
            id(1),
            id(2)
        )),
        "{}",
        ran.stderr
    );
}

#[tokio::test]
async fn resume_held_asks_and_n_moves_nothing() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ran = Box::pin(resume_with(
        terminal(&env),
        &env,
        &["2"],
        Vec::new(),
        vec![Ok(held())],
        Some((Arc::clone(&keys), b'n')),
    ))
    .await;

    assert_eq!(ran.exit, Exit::Success);
    assert_eq!(ran.resumes.len(), 1, "n sends nothing more");
    assert_eq!(
        ran.stdout,
        "\"why is the build slow\" is open in /dev/pts/4. Move it here? [y/N]\n"
    );
    assert_eq!(ran.stderr, "nothing moved\n");
}

#[tokio::test]
async fn resume_held_asks_and_y_takes_it() {
    let env = TestEnv::new();
    let keys = Arc::new(ScriptedKeys::default());
    let ran = Box::pin(resume_with(
        terminal(&env),
        &env,
        &["2"],
        Vec::new(),
        vec![Ok(held()), Ok(resumed(2))],
        Some((Arc::clone(&keys), b'y')),
    ))
    .await;

    assert_eq!(ran.exit, Exit::Success);
    let [first, second] = ran.resumes.try_into().unwrap();
    assert!(!first.take && second.take);
    assert_ne!(first.command_id, second.command_id, "a new command, so no receipt answers it");
    assert!(ran.stdout.ends_with("↪ resumed \"why is the build slow\" (no project, just now)\n"));
}

#[tokio::test]
async fn resume_held_in_a_pipe_needs_take() {
    let env = TestEnv::new();
    let ran =
        Box::pin(resume_with(terminal(&env), &env, &["2"], Vec::new(), vec![Ok(held())], None))
            .await;

    assert_eq!(ran.exit, Exit::DaemonError);
    assert!(
        ran.stderr.contains(
            "\"why is the build slow\" is open in /dev/pts/4; efr resume --take 2 moves it"
        ),
        "{}",
        ran.stderr
    );

    let env = TestEnv::new();
    let ran = Box::pin(resume_with(
        terminal(&env),
        &env,
        &["--take", "2"],
        Vec::new(),
        vec![Ok(resumed(2))],
        None,
    ))
    .await;
    assert_eq!(ran.exit, Exit::Success);
    assert!(ran.resumes[0].take);
}

#[tokio::test]
async fn resume_on_an_older_efrd_says_restart() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let (mut out, captured) = capture();
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        let body = ErrorBody::new(ErrorCode::Invalid, "the frame is not a valid client frame");
        conn.fail(id, body).await;
    };
    let ctx = terminal(&env);
    let line = command(&["resume"]);
    let (exit, ()) = tokio::join!(run::run(&line, &ctx, &mut out), script);

    assert_eq!(exit, Exit::DaemonError);
    assert!(
        captured
            .stderr()
            .contains("efrd is older than this efr; restart it: systemctl --user restart efrd"),
        "{}",
        captured.stderr()
    );
}

#[tokio::test]
async fn resume_of_a_conversation_needs_a_terminal() {
    let env = TestEnv::new();
    let ran =
        Box::pin(resume_with(env.context(), &env, &["2"], Vec::new(), Vec::new(), None)).await;

    assert_eq!(ran.exit, Exit::Usage);
    assert!(ran.stderr.contains("efr resume needs a terminal"), "{}", ran.stderr);
}

/// A context of a terminal on all three streams whose plugin handed over its shell
/// context and, with `words`, the words of `,resume` in `EFR_RESUME_WORDS`; its picker
/// is `picker`.
fn on_a_terminal(env: &TestEnv, picker: &Arc<ScriptedPicker>, words: Option<&str>) -> Context {
    let context = format!(r#"{{"pwd":"/srv","tty":"{TTY}","shell_pid":4242}}"#);
    let mut vars = vec![(Var::Context, context)];
    if let Some(words) = words {
        vars.push((Var::ResumeWords, words.to_owned()));
    }
    Context {
        env: Env::fixed(vars),
        term: terminal_facts(),
        picker: Arc::clone(picker) as Arc<dyn Picker>,
        ..env.context()
    }
}

/// A daemon that searches.
fn searching() -> Daemon {
    Daemon { searching: true, ..Daemon::default() }
}

/// The numbered list as efr prints it on the terminal of `ctx`.
fn numbered_list(ctx: &Context) -> String {
    let options = ctx.render_options(effective_width(ctx.screen.size()));
    resume::list(&recent(), now(), true, &options)
}

#[tokio::test]
async fn without_fzf_the_list_is_as_before() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::none());
    let ctx = on_a_terminal(&env, &picker, None);
    let expected = numbered_list(&ctx);
    let ran = Box::pin(resume_in(ctx, &env, &[], searching())).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(ran.stdout, expected, "byte for byte");
    assert_eq!(picker.finds(), 1);
    assert!(ran.searches.is_empty() && picker.requests().is_empty());
}

#[tokio::test]
async fn an_fzf_that_is_too_old_gives_the_list_and_a_note() {
    let env = TestEnv::new();
    let old = NoPicker::TooOld(Version { major: 0, minor: 44, patch: 1 });
    let picker = Arc::new(ScriptedPicker::failing(old));
    let ctx = on_a_terminal(&env, &picker, None);
    let expected = numbered_list(&ctx);
    let ran = Box::pin(resume_in(ctx, &env, &[], searching())).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let note = ran.stdout.strip_prefix(&expected).expect("the list first");
    assert_eq!(bare(note), "fzf 0.44.1 is older than 0.46, so efr shows the list\n");
}

#[tokio::test]
async fn an_unsafe_path_of_efr_gives_the_list_and_a_note() {
    let env = TestEnv::new();
    let picker = Arc::new(
        ScriptedPicker::running(Script::Outcome(Outcome::Closed))
            .with_efr(Some(PathBuf::from("/home/user/{q}/efr"))),
    );
    let ctx = on_a_terminal(&env, &picker, None);
    let expected = numbered_list(&ctx);
    let ran = Box::pin(resume_in(ctx, &env, &[], searching())).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let note = ran.stdout.strip_prefix(&expected).expect("the list first");
    assert!(bare(note).starts_with("the path of efr or the name of the terminal"), "{note}");
    assert!(picker.requests().is_empty(), "fzf never ran");
}

#[tokio::test]
async fn enter_in_the_picker_resumes_the_row() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Resume(id(2)))));
    let ctx = on_a_terminal(&env, &picker, None);
    let daemon = Daemon { answers: vec![Ok(resumed(2))], ..searching() };
    let ran = Box::pin(resume_in(ctx, &env, &[], daemon)).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(bare(&ran.stdout), "↪ resumed \"why is the build slow\" (no project, just now)\n");
    let [resume] = ran.resumes.try_into().unwrap();
    assert_eq!((resume.conversation_id, resume.take), (id(2), false));
    assert_eq!(resume.context.shell_pid, Some(4242));
    // The first rows are the recent list of the empty query, for this terminal.
    let [search] = ran.searches.try_into().unwrap();
    assert_eq!(search.query, "");
    assert_eq!((search.tty.as_deref(), search.pwd), (Some(TTY), Some(PathBuf::from("/srv"))));
    assert_eq!(search.limit, Some(50));
    let [request] = picker.requests().try_into().unwrap();
    assert_eq!((request.query, request.cwd), (None, PathBuf::from("/srv")));
    let rows = bare(&String::from_utf8(request.rows).unwrap());
    let lines: Vec<&str> = rows.lines().collect();
    assert_eq!(lines.len(), 3, "the header and two rows: {rows}");
    assert!(lines[0].starts_with("\tenter resume"), "{rows}");
    assert!(lines[1].starts_with(&format!("{}\tfix the tests", id(1))), "{rows}");
    let args = fzf_args(&request.look);
    let reload = args.iter().map(|arg| arg.to_string_lossy()).find(|arg| arg.contains("reload"));
    assert_eq!(
        reload.as_deref(),
        Some(
            "--bind=change:first+reload-sync:exec '/usr/bin/efr' search --picker --tty '/dev/pts/7'"
        )
    );
}

#[tokio::test]
async fn ctrl_d_in_the_picker_asks_deletes_and_opens_it_again_with_the_query() {
    let env = TestEnv::new();
    let delete = Outcome::Delete { id: id(2), query: "slow".to_owned() };
    let picker = Arc::new(ScriptedPicker::running(Script::Outcomes(vec![delete])));
    let keys = Arc::new(ScriptedKeys::default());
    let ctx =
        Context { keys: Arc::clone(&keys) as Arc<dyn Keys>, ..on_a_terminal(&env, &picker, None) };
    let deleted = ConversationDeleteResult::Deleted {
        conversation_id: id(2),
        title: Some("why is the build slow".to_owned()),
        jobs_stopped: 0,
        not_removed: Vec::new(),
    };
    let daemon = Daemon {
        listed: vec![summary(1, "fix the tests"), summary(2, "why is the build slow")],
        deletes: vec![Ok(deleted)],
        ..searching()
    };
    let answer = async {
        keys.press(b'y').await;
    };
    let (ran, ()) = tokio::join!(Box::pin(resume_in(ctx, &env, &[], daemon)), answer);

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(
        bare(&ran.stdout),
        "Delete \"why is the build slow\"? Nothing takes a delete back. [y/N]\n\
         deleted \"why is the build slow\"\n"
    );
    let [delete] = ran.deletes.try_into().unwrap();
    assert_eq!((delete.conversation_id, delete.force, delete.stop), (id(2), false, false));
    // The picker opens again with the query that it had, and new rows.
    let [first, again] = picker.requests().try_into().unwrap();
    assert_eq!((first.query, again.query.as_deref()), (None, Some("slow")));
    let queries: Vec<&str> = ran.searches.iter().map(|search| search.query.as_str()).collect();
    assert_eq!(queries, ["", "slow"]);
    assert!(ran.resumes.is_empty());
}

#[tokio::test]
async fn ctrl_d_and_n_deletes_nothing_and_opens_the_picker_again() {
    let env = TestEnv::new();
    let delete = Outcome::Delete { id: id(2), query: String::new() };
    let picker = Arc::new(ScriptedPicker::running(Script::Outcomes(vec![delete])));
    let keys = Arc::new(ScriptedKeys::default());
    let ctx =
        Context { keys: Arc::clone(&keys) as Arc<dyn Keys>, ..on_a_terminal(&env, &picker, None) };
    let daemon = Daemon { listed: vec![summary(2, "why is the build slow")], ..searching() };
    let answer = async {
        keys.press(b'n').await;
    };
    let (ran, ()) = tokio::join!(Box::pin(resume_in(ctx, &env, &[], daemon)), answer);

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert!(ran.deletes.is_empty());
    assert_eq!(bare(&ran.stderr), "nothing was deleted\n");
    assert_eq!(picker.requests().len(), 2);
}

#[tokio::test]
async fn ctrl_o_prints_the_conversation_and_moves_nothing() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Print(id(1)))));
    let ctx = on_a_terminal(&env, &picker, None);
    let ran = Box::pin(resume_in(ctx, &env, &[], searching())).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert!(ran.resumes.is_empty(), "no switch");
    let [history] = ran.histories.try_into().unwrap();
    assert_eq!((history.conversation_id, history.cursor), (id(1), None));
    assert_eq!(bare(&ran.stdout), format!("conversation {}\n", id(1)));
}

#[tokio::test]
async fn esc_closes_the_picker_and_changes_nothing() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Closed)));
    let ctx = on_a_terminal(&env, &picker, None);
    let ran = Box::pin(resume_in(ctx, &env, &[], searching())).await;

    assert_eq!((ran.exit, ran.stdout.as_str(), ran.stderr.as_str()), (Exit::Success, "", ""));
    assert!(ran.resumes.is_empty() && ran.histories.is_empty());
}

#[tokio::test]
async fn ctrl_c_in_the_picker_exits_130_with_no_message() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::UntilStopped));
    let interrupt = Arc::new(TestInterrupt::default());
    let ctx = Context {
        interrupt: Arc::clone(&interrupt) as Arc<dyn Interrupt>,
        ..on_a_terminal(&env, &picker, None)
    };
    let press = async {
        picker.until_running().await;
        interrupt.trigger();
    };
    let (ran, ()) = tokio::join!(Box::pin(resume_in(ctx, &env, &[], searching())), press);

    assert_eq!((ran.exit, ran.stdout.as_str(), ran.stderr.as_str()), (Exit::Interrupted, "", ""));
    assert!(ran.resumes.is_empty());
}

#[tokio::test]
async fn sigterm_ends_the_picker_with_the_signal() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::UntilStopped));
    let terminate = Arc::new(TestTerminate::default());
    let ctx = Context {
        terminate: Arc::clone(&terminate) as Arc<dyn Terminate>,
        ..on_a_terminal(&env, &picker, None)
    };
    let send = async {
        picker.until_running().await;
        terminate.trigger();
    };
    let (ran, ()) = tokio::join!(Box::pin(resume_in(ctx, &env, &[], searching())), send);

    assert_eq!(ran.exit, Exit::Signal(TestTerminate::SIGTERM));
    assert_eq!(ran.stderr, "");
}

#[tokio::test]
async fn a_failed_fzf_gives_the_list_and_its_status() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Failed(Some(2)))));
    let ctx = on_a_terminal(&env, &picker, None);
    let expected = numbered_list(&ctx);
    let ran = Box::pin(resume_in(ctx, &env, &[], searching())).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let note = ran.stdout.strip_prefix(&expected).expect("the list first");
    assert_eq!(bare(note), "fzf ended with status 2, so efr shows the list\n");
}

#[tokio::test]
async fn a_held_conversation_from_the_picker_asks_and_n_moves_nothing() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Resume(id(2)))));
    let keys = Arc::new(ScriptedKeys::default());
    let ctx = on_a_terminal(&env, &picker, None);
    let daemon =
        Daemon { answers: vec![Ok(held())], keys: Some((Arc::clone(&keys), b'n')), ..searching() };
    let ran = Box::pin(resume_in(ctx, &env, &[], daemon)).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(ran.resumes.len(), 1, "n sends nothing more");
    assert!(bare(&ran.stdout).contains("Move it here? [y/N]"), "{}", ran.stdout);
    assert_eq!(bare(&ran.stderr), "nothing moved\n");
}

#[tokio::test]
async fn an_efrd_that_does_not_search_gives_the_list() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Closed)));
    let ctx = on_a_terminal(&env, &picker, None);
    let expected = numbered_list(&ctx);
    let ran = Box::pin(resume_in(ctx, &env, &[], Daemon::default())).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(ran.stdout, expected);
    assert!(picker.requests().is_empty() && ran.searches.is_empty());
}

/// A change of a context.
type Change = fn(Context) -> Context;

/// A change of the terminal facts of `ctx`.
fn with_term(ctx: Context, change: impl FnOnce(&mut TermFacts)) -> Context {
    let mut term = ctx.term.clone();
    change(&mut term);
    Context { term, ..ctx }
}

#[tokio::test]
async fn a_pipe_a_dumb_terminal_and_the_list_setting_never_look_for_fzf() {
    let cases: [(&str, Change); 4] = [
        ("stdout in a pipe", |ctx| with_term(ctx, |term| term.stdout_tty = false)),
        ("TERM=dumb", |ctx| with_term(ctx, |term| term.term = Some("dumb".to_owned()))),
        ("TERM unset", |ctx| with_term(ctx, |term| term.term = None)),
        ("resume.picker = list", |ctx| {
            let text = "[resume]\npicker = \"list\"\n";
            let settings = Settings::parse(Path::new("/home/user/.config/efr/config.toml"), text);
            assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
            Context { settings, ..ctx }
        }),
    ];
    for (case, change) in cases {
        let env = TestEnv::new();
        let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Closed)));
        let ctx = change(on_a_terminal(&env, &picker, None));
        let expected = numbered_list(&ctx);
        let ran = Box::pin(resume_in(ctx, &env, &[], searching())).await;

        assert_eq!(ran.exit, Exit::Success, "{case}: {}", ran.stderr);
        assert_eq!(ran.stdout, expected, "{case}");
        assert_eq!(picker.finds(), 0, "{case}");
    }
}

#[tokio::test]
async fn words_from_the_plugin_open_the_picker_with_them_as_the_query() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Closed)));
    let ctx = on_a_terminal(&env, &picker, Some("rsync laptop"));
    let ran = Box::pin(resume_in(ctx, &env, &[], searching())).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let [search] = ran.searches.try_into().unwrap();
    assert_eq!((search.query.as_str(), search.limit), ("rsync laptop", Some(50)));
    let [request] = picker.requests().try_into().unwrap();
    assert_eq!(request.query.as_deref(), Some("rsync laptop"));
}

#[tokio::test]
async fn words_typed_by_hand_win_over_the_variable() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Closed)));
    let ctx = on_a_terminal(&env, &picker, Some("from the plugin"));
    let ran = Box::pin(resume_in(ctx, &env, &["docker", "compose"], searching())).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(picker.requests()[0].query.as_deref(), Some("docker compose"));
}

#[tokio::test]
async fn words_without_the_picker_print_the_best_matches() {
    let env = TestEnv::new();
    let ctx = Context {
        env: Env::fixed(vec![
            (Var::Context, format!(r#"{{"pwd":"/srv","tty":"{TTY}"}}"#)),
            (Var::ResumeWords, "build slow".to_owned()),
        ]),
        ..env.context()
    };
    let ran = Box::pin(resume_in(ctx, &env, &[], searching())).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let [search] = ran.searches.try_into().unwrap();
    assert_eq!((search.query.as_str(), search.limit), ("build slow", Some(10)));
    assert!(ran.stdout.starts_with("fix the tests"), "{}", ran.stdout);
    assert!(ran.stdout.ends_with(",resume <id> continues one here\n"), "{}", ran.stdout);
    assert!(ran.resumes.is_empty() && ran.recent.is_empty());
}

#[tokio::test]
async fn an_id_start_from_the_plugin_that_matches_one_conversation_resumes_it() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Closed)));
    let ctx = on_a_terminal(&env, &picker, Some("019a9b1c"));
    let daemon = Daemon {
        listed: vec![summary(2, "why is the build slow")],
        answers: vec![Ok(resumed(2))],
        ..searching()
    };
    let ran = Box::pin(resume_in(ctx, &env, &[], daemon)).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let [resume] = ran.resumes.try_into().unwrap();
    assert_eq!(resume.conversation_id, id(2));
    assert!(picker.requests().is_empty() && ran.searches.is_empty());
}

#[tokio::test]
async fn a_hex_word_that_starts_no_id_is_a_search() {
    let env = TestEnv::new();
    let picker = Arc::new(ScriptedPicker::running(Script::Outcome(Outcome::Closed)));
    let ctx = on_a_terminal(&env, &picker, Some("cafe"));
    let daemon = Daemon { listed: vec![summary(2, "why is the build slow")], ..searching() };
    let ran = Box::pin(resume_in(ctx, &env, &[], daemon)).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    assert_eq!(ran.searches[0].query, "cafe");
    assert_eq!(picker.requests()[0].query.as_deref(), Some("cafe"));
}

#[tokio::test]
async fn take_without_n_or_an_id_is_a_usage_error() {
    let env = TestEnv::new();
    let (mut out, captured) = capture();
    let exit = run::run(&command(&["resume", "--take"]), &terminal(&env), &mut out).await;

    assert_eq!(exit, Exit::Usage);
    let stderr = captured.stderr();
    assert!(stderr.contains("efr resume --take needs N or a conversation id"), "{stderr}");
}

#[tokio::test]
async fn take_with_n_from_the_plugin_moves_it() {
    let env = TestEnv::new();
    let context = format!(r#"{{"pwd":"/srv","tty":"{TTY}","shell_pid":4242}}"#);
    let ctx = Context {
        env: Env::fixed(vec![(Var::Context, context), (Var::ResumeWords, "2".to_owned())]),
        ..env.context()
    };
    let ran =
        Box::pin(resume_with(ctx, &env, &["--take"], Vec::new(), vec![Ok(resumed(2))], None)).await;

    assert_eq!(ran.exit, Exit::Success, "{}", ran.stderr);
    let [resume] = ran.resumes.try_into().unwrap();
    assert_eq!((resume.conversation_id, resume.take), (id(2), true));
}

#[test]
fn the_fake_daemon_does_not_search_unless_asked() {
    assert_eq!(hello_result().capabilities.search, None);
}
