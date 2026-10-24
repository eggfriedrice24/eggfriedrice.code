use std::path::{Path, PathBuf};
use std::sync::Arc;

use efr_protocol::{
    ConversationId, ConversationSearch, ConversationSearchResult, ConversationStatus,
    ConversationSummary, ConversationsSearch, ConversationsSearchResult, ErrorBody, ErrorCode,
    Method, RecentConversation, RouteScope, SearchLine, SearchRow, Seq, TextRange,
};
use efr_stdx::env::{Env, Var};
use efr_stdx::time::{Clock, Sleep};
use jiff::Timestamp;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::{PICKER_WAIT, PREVIEW_WAIT, check, terms};
use crate::context::{Context, FzfQuery};
use crate::error::{CliError, Exit};
use crate::run;
use crate::settings::Settings;
use crate::testing::{GateClock, TestEnv, bare, capture, command, now};

const TTY: &str = "/dev/pts/9";

fn id(n: u64) -> ConversationId {
    format!("019a9b1c-3d00-7a10-8b20-{n:012x}").parse().unwrap()
}

fn row(n: u64, title: &str) -> SearchRow {
    SearchRow {
        conversation: RecentConversation {
            summary: ConversationSummary {
                id: id(n),
                title: Some(title.to_owned()),
                status: ConversationStatus::Idle,
                created_at: now(),
                updated_at: now(),
                last_seq: Seq::new(n),
                cwd: None,
                scope: None,
                tty: None,
            },
            route: RouteScope::Outside,
            project: None,
            turns: 2,
            open_in: None,
            here: false,
        },
        short_id: format!("019a9b1{n}"),
        title_ranges: vec![TextRange { start: 0, end: 5 }],
        hit: None,
    }
}

fn found() -> ConversationsSearchResult {
    ConversationsSearchResult {
        rows: vec![row(1, "rsync the photos"), row(2, "rsync the music")],
        ..Default::default()
    }
}

/// A clock whose sleeps of the hidden forms' waits finish at once, and whose longer
/// sleeps, such as the time limit of hello, never do.
#[derive(Debug)]
struct ShortSleeps;

impl Clock for ShortSleeps {
    fn now(&self) -> Timestamp {
        now()
    }

    fn sleep(&self, duration: std::time::Duration) -> Sleep {
        if duration <= PICKER_WAIT {
            return Box::pin(std::future::ready(()));
        }
        Box::pin(std::future::pending())
    }
}

/// A context of fzf's child: the query in `FZF_QUERY`, the gate clock, the working
/// directory of the picker.
fn in_fzf(env: &TestEnv, query: &str, clock: Arc<dyn Clock>) -> Context {
    Context {
        fzf_query: Some(FzfQuery(query.to_owned())),
        clock,
        cwd: Some(PathBuf::from("/srv/app")),
        ..env.context()
    }
}

/// What a run of `efr <args>` printed.
struct Ran {
    exit: Exit,
    stdout: String,
    stderr: String,
}

/// Runs `efr <args>` with `ctx` while `script` plays the daemon.
async fn run_with(ctx: &Context, args: &[&str], script: impl Future<Output = ()>) -> Ran {
    let (mut out, captured) = capture();
    let line = command(args);
    let (exit, ()) = tokio::join!(Box::pin(run::run(&line, ctx, &mut out)), script);
    Ran { exit, stdout: bare(&captured.stdout()), stderr: captured.stderr() }
}

#[test]
fn terms_are_counted_as_efrd_counts_them() {
    assert_eq!(terms(""), 0);
    assert_eq!(terms("  rsync   lap "), 2);
    assert_eq!(terms("\"to the laptop\" rsync"), 2);
    assert_eq!(terms("say\"open phrase"), 2, "a quote starts a phrase inside a word");
    assert_eq!(terms("\"\" \"  \" x"), 1, "an empty phrase is no word");
    assert!(check(&"a ".repeat(16)).is_ok());
    assert!(matches!(check(&"a ".repeat(17)), Err(CliError::QueryTooManyWords { words: 17 })));
    assert!(check(&"x".repeat(256)).is_ok());
    assert!(matches!(check(&"é".repeat(257)), Err(CliError::QueryTooLong { chars: 257 })));
}

#[tokio::test]
async fn picker_waits_then_asks_with_the_query_the_terminal_and_the_directory() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let clock = Arc::new(GateClock::default());
    let ctx = in_fzf(&env, "rsync lap", Arc::clone(&clock) as _);
    let script = async {
        // The wait comes before the connection: a key in this time ends the process
        // before it reaches efrd.
        tokio::select! {
            biased;
            _ = daemon.accept() => panic!("efr connected before its wait"),
            () = clock.until_slept(PICKER_WAIT) => {}
        }
        clock.open();
        let mut conn = daemon.accept().await;
        assert_eq!(conn.hello().tty.as_deref(), Some(TTY));
        let (id, method) = conn.request().await;
        let Method::ConversationsSearch(params) = method else {
            panic!("expected conversations.search, got {}", method.name());
        };
        assert_eq!(
            params,
            ConversationsSearch {
                query: "rsync lap".to_owned(),
                tty: Some(TTY.to_owned()),
                pwd: Some(PathBuf::from("/srv/app")),
                limit: Some(50),
                stats: false,
            }
        );
        conn.reply(id, &found()).await;
        conn.until_closed().await;
    };
    let ran = run_with(&ctx, &["search", "--picker", "--tty", TTY], script).await;

    assert_eq!(ran.exit, Exit::Success);
    assert_eq!(ran.stderr, "");
    let lines: Vec<&str> = ran.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", ran.stdout);
    assert_eq!(lines[0], "\tenter resume · ctrl-o print · ctrl-d delete · esc close");
    assert!(lines[1].starts_with(&format!("{}\trsync the photos", id(1))), "{}", lines[1]);
}

/// Runs `efr search --picker` against a daemon that fails the search with `body`.
async fn picker_failing_with(body: ErrorBody) -> Ran {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = in_fzf(&env, "x", Arc::new(ShortSleeps));
    let script = async {
        let mut conn = daemon.accept().await;
        let (id, _) = conn.request().await;
        conn.fail(id, body).await;
        conn.until_closed().await;
    };
    run_with(&ctx, &["search", "--picker", "--tty", TTY], script).await
}

#[tokio::test]
async fn picker_failure_becomes_the_note_of_the_header() {
    let body = ErrorBody::new(ErrorCode::Internal, "the search failed in the store");
    let ran = picker_failing_with(body).await;

    assert_eq!((ran.exit, ran.stderr.as_str()), (Exit::Success, ""));
    assert_eq!(
        ran.stdout,
        "\tenter resume · ctrl-o print · ctrl-d delete · esc close   the daemon failed the request with internal: the search failed in the store\n"
    );
}

#[tokio::test]
async fn busy_efrd_gives_an_empty_list_with_no_note() {
    let ran = picker_failing_with(ErrorBody::new(ErrorCode::Busy, "no search lane")).await;

    assert_eq!((ran.exit, ran.stderr.as_str()), (Exit::Success, ""));
    assert_eq!(ran.stdout, "\tenter resume · ctrl-o print · ctrl-d delete · esc close\n");
}

#[tokio::test]
async fn older_efrd_says_to_restart_it_in_the_header() {
    let body = ErrorBody::new(ErrorCode::Invalid, "the frame is not a valid client frame");
    let ran = picker_failing_with(body).await;

    assert_eq!((ran.exit, ran.stderr.as_str()), (Exit::Success, ""));
    assert!(
        ran.stdout.ends_with(
            "   efrd is older than this efr; restart it: systemctl --user restart efrd\n"
        ),
        "{}",
        ran.stdout
    );
}

#[tokio::test]
async fn without_efrd_the_header_says_that_efrd_does_not_answer() {
    let env = TestEnv::new();
    let ctx = Context { clock: Arc::new(ShortSleeps), ..env.context() };
    let ran = run_with(&ctx, &["search", "--picker", "--tty", TTY], async {}).await;

    assert_eq!((ran.exit, ran.stderr.as_str()), (Exit::Success, ""));
    assert_eq!(
        ran.stdout,
        "\tenter resume · ctrl-o print · ctrl-d delete · esc close   efrd does not answer\n"
    );
}

#[tokio::test]
async fn a_config_warning_never_reaches_stderr_in_the_hidden_forms() {
    let env = TestEnv::new();
    let settings = Settings::parse(Path::new("/c/config.toml"), "[render\n");
    assert!(!settings.warnings.is_empty());
    let ctx = Context { clock: Arc::new(ShortSleeps), settings, ..env.context() };
    let picker = run_with(&ctx, &["search", "--picker", "--tty", TTY], async {}).await;
    let preview =
        run_with(&ctx, &["search", "--preview", &id(1).to_string(), "--tty", TTY], async {}).await;
    let list = run_with(&ctx, &["search", "rsync"], async {}).await;

    assert_eq!((picker.stderr.as_str(), preview.stderr.as_str()), ("", ""));
    assert_eq!(preview.stdout, "efrd does not answer\n");
    assert!(list.stderr.contains("efr: warning: "), "the plain list warns: {}", list.stderr);
    assert_eq!(list.exit, Exit::NotRunning);
}

#[tokio::test]
async fn a_query_over_256_characters_fails_before_a_call() {
    let env = TestEnv::new();
    let long = "x".repeat(257);
    let ran = run_with(&env.context(), &["search", &long], async {}).await;
    assert_eq!(ran.exit, Exit::Usage, "no call, so no missing daemon: {}", ran.stderr);
    assert_eq!(ran.stderr, "efr: the query is longer than 256 characters\n");

    let ctx = in_fzf(&env, &long, Arc::new(GateClock::default()));
    let ran = run_with(&ctx, &["search", "--picker", "--tty", TTY], async {}).await;
    assert_eq!((ran.exit, ran.stderr.as_str()), (Exit::Success, ""));
    assert_eq!(
        ran.stdout,
        "\tenter resume · ctrl-o print · ctrl-d delete · esc close   the query is longer than 256 characters\n"
    );
}

#[tokio::test]
async fn search_without_words_is_a_usage_error_that_names_resume() {
    let env = TestEnv::new();
    let ran = run_with(&env.context(), &["search", "--", "  "], async {}).await;

    assert_eq!(ran.exit, Exit::Usage);
    assert_eq!(
        ran.stderr,
        "efr: efr search needs the words to find\nefr: ,resume in zsh, or efr resume, lists the recent conversations\n"
    );
}

#[tokio::test]
async fn preview_of_an_empty_id_prints_nothing() {
    let env = TestEnv::new();
    let ran = run_with(&env.context(), &["search", "--preview", "", "--tty", TTY], async {}).await;

    assert_eq!(ran.exit, Exit::Success);
    assert_eq!((ran.stdout.as_str(), ran.stderr.as_str()), ("", ""));
}

#[tokio::test]
async fn preview_waits_then_asks_for_the_hits_of_one_conversation() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let clock = Arc::new(GateClock::default());
    let ctx =
        Context { preview_columns: Some(40), ..in_fzf(&env, "rsync", Arc::clone(&clock) as _) };
    let script = async {
        tokio::select! {
            biased;
            _ = daemon.accept() => panic!("efr connected before its wait"),
            () = clock.until_slept(PREVIEW_WAIT) => {}
        }
        clock.open();
        let mut conn = daemon.accept().await;
        let (request, method) = conn.request().await;
        let Method::ConversationSearch(params) = method else {
            panic!("expected conversation.search, got {}", method.name());
        };
        assert_eq!(
            params,
            ConversationSearch { conversation_id: id(2), query: "rsync".to_owned(), limit: None }
        );
        let result = ConversationSearchResult {
            title: Some(SearchLine {
                text: "a title much longer than the forty columns of this preview".to_owned(),
                ranges: Vec::new(),
            }),
            ..Default::default()
        };
        conn.reply(request, &result).await;
        conn.until_closed().await;
    };
    let ran =
        run_with(&ctx, &["search", "--preview", &id(2).to_string(), "--tty", TTY], script).await;

    assert_eq!((ran.exit, ran.stderr.as_str()), (Exit::Success, ""));
    assert_eq!(
        ran.stdout,
        "a title much longer than the forty colu…\n\nno prompt, answer or command matches\n"
    );
}

#[tokio::test]
async fn plain_list_asks_for_ten_rows_and_json_for_the_measures_too() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let context = format!(r#"{{"pwd":"/srv","tty":"{TTY}","shell_pid":4242}}"#);
    let ctx = Context { env: Env::fixed(vec![(Var::Context, context)]), ..env.context() };
    let script = async {
        for (want_limit, want_stats) in [(10, false), (5, true)] {
            let mut conn = daemon.accept().await;
            let (id, method) = conn.request().await;
            let Method::ConversationsSearch(params) = method else {
                panic!("expected conversations.search, got {}", method.name());
            };
            assert_eq!(params.query, "rsync \"the lap\"");
            assert_eq!((params.limit, params.stats), (Some(want_limit), want_stats));
            assert_eq!(
                (params.tty.as_deref(), params.pwd),
                (Some(TTY), Some(PathBuf::from("/srv")))
            );
            let mut answer = serde_json::to_value(found()).unwrap();
            answer["stats"] = json!({"rows_scanned": 3, "conversations_scored": 2, "titles_scored": 2, "elapsed_us": 900});
            answer["from_a_newer_efrd"] = json!(true);
            conn.reply(id, &answer).await;
            conn.until_closed().await;
        }
    };
    let (mut out, captured) = capture();
    let (list, json_run) = (
        command(&["search", "rsync", "\"the lap\""]),
        command(&["search", "--json", "--limit", "5", "--", "rsync", "\"the lap\""]),
    );
    let runs = async {
        let first = Box::pin(run::run(&list, &ctx, &mut out)).await;
        let first_out = captured.stdout();
        let second = Box::pin(run::run(&json_run, &ctx, &mut out)).await;
        (first, first_out, second)
    };
    let ((first, listed, second), ()) = tokio::join!(runs, script);

    assert_eq!((first, second), (Exit::Success, Exit::Success), "{}", captured.stderr());
    assert!(listed.starts_with("rsync the photos"), "{listed}");
    assert!(listed.ends_with(",resume <id> continues one here\n"), "the plugin's hint: {listed}");
    let printed: Value = serde_json::from_str(&captured.stdout()[listed.len()..]).unwrap();
    assert_eq!(printed["from_a_newer_efrd"], json!(true), "the answer as it came");
    assert_eq!(printed["stats"]["elapsed_us"], json!(900));
}

#[test]
fn the_hidden_forms_are_quiet_and_the_list_is_not() {
    assert!(command(&["search", "--picker", "--tty", TTY]).is_quiet());
    assert!(command(&["search", "--preview", "x", "--tty", TTY]).is_quiet());
    assert!(!command(&["search", "rsync"]).is_quiet());
}
