//! `conversations.search` and `conversation.search` over a `TestDaemon`: what a search
//! finds and never finds, the titles that its rows show, its caps and its stop, its
//! lanes apart from the turns, who may search, and what it logs.
//!
//! Most tests start a persistent daemon on a store that the test wrote before the
//! start ([`seeded`]), so each test knows every text of the log; the daemon's indexer
//! backfills it at its start, and [`indexed`] moves the clock through the pauses of the
//! backfill. The tests that hold a search use the daemon's hook in the row loop
//! ([`Hold`]), on a file database: an in-memory store searches on its writer.

use std::path::Path;
use std::str::FromStr;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use efr_daemon::SearchRowHook;
use efr_protocol::{
    CallId, CommandId, ConversationId, ConversationSearch, ConversationSearchResult,
    ConversationsRecent, ConversationsRecentResult, ConversationsSearch, ConversationsSearchResult,
    ErrorCode, Event, Method, Origin, PageCursor, PromptSendResult, Scope, SearchKind, SearchLine,
    SearchPartial, Seq, ShellContext, TurnId, TurnSettings,
};
use efr_stdx::time::Clock as _;
use efr_store::{Batch, Store, StoreConfig};
use efr_test_daemon::{Client, ClientError, ItemStream, TTY, TestDaemon, TestDaemonBuilder};
use efr_test_support::{TestClock, TestDirs};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use serde_json::json;
use tokio::sync::mpsc as async_mpsc;

use crate::routing::Place;
use crate::search_index::{FORMS, secret_text};
use crate::support::{logged, logs};

/// When a search ranks what it has seen.
const SOFT_CAP: Duration = Duration::from_millis(150);

/// When efrd answers a search that has not ended.
const HARD_CAP: Duration = Duration::from_millis(200);

/// An id of type `T` from a number.
fn id<T: FromStr>(n: u64) -> T
where
    T::Err: std::fmt::Debug,
{
    format!("0192f0c1-7a00-7000-8000-{n:012x}").parse().unwrap()
}

/// The texts of one turn of a seeded store.
#[derive(Debug, Clone, Default)]
struct Turn {
    prompt: String,
    steer: Option<String>,
    answer: Option<String>,
    command: Option<String>,
    output: Option<String>,
}

impl Turn {
    fn prompt(text: &str) -> Turn {
        Turn { prompt: text.to_owned(), ..Turn::default() }
    }
}

/// A batch of the turn `n` of `conversation`, which `create` starts.
fn turn_batch(conversation: ConversationId, n: u64, create: bool, turn: &Turn) -> Batch {
    let turn_id: TurnId = id(1_000_000 + n);
    let call_id: CallId = id(2_000_000 + n);
    let mut events = Vec::new();
    if create {
        events.push(Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None });
    }
    events.push(Event::PromptQueued {
        turn_id,
        command_id: id::<CommandId>(3_000_000 + n),
        text: turn.prompt.clone(),
        origin: Origin::Shell,
        context: Some(ShellContext::new("/srv/p")),
        settings: TurnSettings::default(),
        steers: Vec::new(),
    });
    events.push(Event::TurnStarted {
        turn_id,
        cwd: "/srv/p".into(),
        scope: Scope::Path("/srv/p".into()),
        settings: None,
    });
    if let Some(steer) = &turn.steer {
        events.push(Event::TurnSteered { turn_id, text: steer.clone() });
    }
    if let Some(answer) = &turn.answer {
        events.push(Event::AssistantMessageCompleted { turn_id, index: 0, text: answer.clone() });
    }
    if let Some(command) = &turn.command {
        events.push(Event::ToolCallStarted {
            turn_id,
            call_id,
            tool: "shell".to_owned(),
            input: json!({ "command": command }),
            freeform: false,
            manual_input: false,
            launch: None,
        });
        let completed = json!({
            "kind": "tool_call_completed",
            "turn_id": turn_id,
            "call_id": call_id,
            "output": turn.output.clone().unwrap_or_default(),
            "truncated": false,
            "is_error": false,
            "exit_code": 0,
        });
        events.push(serde_json::from_value(completed).unwrap());
    }
    events.push(Event::TurnCompleted {
        turn_id,
        usage: None,
        context: None,
        changes: None,
        stop: None,
        undo: None,
    });
    events.into_iter().fold(Batch::new(), |batch, event| batch.event(conversation, event))
}

/// The batches of conversations, one per entry of `turns`, each with the turns given:
/// the conversation `n` is `id(n + 1)`.
fn conversations(turns: &[Vec<Turn>]) -> Vec<Batch> {
    let mut batches = Vec::new();
    let mut n = 0;
    for (index, list) in turns.iter().enumerate() {
        let conversation = id(index as u64 + 1);
        for (at, turn) in list.iter().enumerate() {
            n += 1;
            batches.push(turn_batch(conversation, n, at == 0, turn));
        }
    }
    batches
}

/// Writes `batches` into the store of a new tree, a minute apart; the tree and the
/// newest sequence number.
async fn seed(batches: Vec<Batch>) -> (Arc<TestDirs>, Seq) {
    let dirs = Arc::new(TestDirs::new().unwrap());
    let clock = TestClock::new();
    let store =
        Store::open(StoreConfig::in_data_dir(dirs.dirs().data()), clock.shared()).await.unwrap();
    for batch in batches {
        clock.advance(Duration::from_secs(60));
        store.writer().append(batch).await.unwrap();
    }
    let newest = store.readers().with(efr_store::events::last_seq).await.unwrap();
    store.close().await;
    (dirs, newest)
}

/// A persistent daemon on a store that holds `batches` from before its start, with
/// every event in the search index.
async fn seeded(
    batches: Vec<Batch>,
    change: impl FnOnce(TestDaemonBuilder) -> TestDaemonBuilder,
) -> TestDaemon {
    let (dirs, newest) = seed(batches).await;
    let daemon = change(TestDaemon::builder().dirs(dirs).persistent()).start().await.unwrap();
    indexed(&daemon, newest).await;
    daemon
}

/// Moves the daemon's clock through the debounce and the pauses of its indexer until
/// every event up to `seq` is in the index.
async fn indexed(daemon: &TestDaemon, seq: Seq) {
    let clock = daemon.clock();
    let mut status = daemon.search_index();
    loop {
        let now = clock.now();
        let seen = *status
            .wait_for(|status| {
                status.indexed_seq >= seq || status.sleeping_until.is_some_and(|until| until > now)
            })
            .await
            .unwrap();
        if seen.indexed_seq >= seq {
            return;
        }
        let until = seen.sleeping_until.unwrap();
        clock.advance(Duration::try_from(until.duration_since(now)).unwrap());
    }
}

fn list_params(query: &str) -> ConversationsSearch {
    ConversationsSearch { query: query.to_owned(), limit: Some(200), ..Default::default() }
}

async fn search(client: &Client, query: &str) -> ConversationsSearchResult {
    client.call(Method::ConversationsSearch(list_params(query))).await.unwrap()
}

async fn search_in(
    client: &Client,
    conversation_id: ConversationId,
    query: &str,
) -> ConversationSearchResult {
    let params = ConversationSearch { conversation_id, query: query.to_owned(), limit: None };
    client.call(Method::ConversationSearch(params)).await.unwrap()
}

fn ids(result: &ConversationsSearchResult) -> Vec<ConversationId> {
    result.rows.iter().map(|row| row.conversation.summary.id).collect()
}

/// The text of `line` in each of its ranges.
fn marked(line: &SearchLine) -> Vec<String> {
    line.ranges
        .iter()
        .map(|range| {
            line.text.chars().skip(range.start as usize).take((range.end - range.start) as usize)
        })
        .map(Iterator::collect)
        .collect()
}

/// Every text that a list answer shows: the titles and the snippet lines.
fn list_texts(result: &ConversationsSearchResult) -> Vec<String> {
    let mut texts = Vec::new();
    for row in &result.rows {
        texts.extend(row.conversation.summary.title.clone());
        if let Some(hit) = &row.hit {
            texts.extend(hit.lines.iter().map(|line| line.text.clone()));
        }
    }
    texts
}

/// Every text that a preview answer shows.
fn preview_texts(result: &ConversationSearchResult) -> Vec<String> {
    let mut texts: Vec<String> = result.title.iter().map(|line| line.text.clone()).collect();
    for hit in &result.hits {
        texts.extend(hit.lines.iter().map(|line| line.text.clone()));
    }
    texts
}

/// The code and the message of a refused call.
fn refused<T: std::fmt::Debug>(answer: Result<T, ClientError>) -> (ErrorCode, String) {
    match answer {
        Err(ClientError::Server { body }) => (body.code, body.message),
        other => panic!("not refused: {other:?}"),
    }
}

#[tokio::test]
async fn hello_says_search() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    assert_eq!(client.hello().capabilities.search, Some(true));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn phone_may_search() {
    let daemon = seeded(conversations(&[vec![Turn::prompt("sync the photos")]]), |b| b).await;
    let phone = daemon.connect(daemon.connect_options_from(Origin::Phone)).await.unwrap();

    let found = search(&phone, "photos").await;

    assert_eq!(ids(&found), [id(1)]);
    let preview = search_in(&phone, id(1), "photos").await;
    assert_eq!(preview.hits.len(), 1);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn query_finds_prompt_answer_and_command() {
    let rsync = Turn {
        prompt: "sync the holiday photos".to_owned(),
        answer: Some("use rsync to copy them".to_owned()),
        command: Some("rsync -av ~/photos laptop:/backup".to_owned()),
        ..Turn::default()
    };
    let other = Turn::prompt("bake a cake");
    let daemon = seeded(conversations(&[vec![rsync], vec![other]]), |b| b).await;
    let client = daemon.client().await.unwrap();

    for (query, kind, word) in [
        ("holiday", SearchKind::Prompt, "holiday"),
        ("laptop", SearchKind::Command, "laptop"),
        ("copy", SearchKind::Answer, "copy"),
    ] {
        let found = search(&client, query).await;
        assert_eq!(ids(&found), [id(1)], "{query}");
        let hit = found.rows[0].hit.as_ref().unwrap();
        assert_eq!(hit.kind, kind, "{query}");
        assert_eq!(marked(&hit.lines[0]), [word], "{query}");
        // The history page of the cursor ends at the hit.
        assert_eq!(hit.history_cursor, PageCursor::new(format!("s:{}", hit.seq.get() + 1)));
        assert!(found.rows[0].short_id.len() >= 8);
        assert!(id::<ConversationId>(1).to_string().starts_with(&found.rows[0].short_id));
    }
    let preview = search_in(&client, id(1), "rsync").await;
    let kinds: Vec<SearchKind> = preview.hits.iter().map(|hit| hit.kind).collect();
    assert_eq!(kinds, [SearchKind::Command, SearchKind::Answer], "newest first");
    assert_eq!(preview.title.unwrap().text, "sync the holiday photos");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn query_never_finds_command_output() {
    let turn = Turn {
        prompt: "run the tests".to_owned(),
        command: Some("make test".to_owned()),
        output: Some("zebraoutput failed in the suite".to_owned()),
        ..Turn::default()
    };
    let daemon = seeded(conversations(&[vec![turn]]), |b| b).await;
    let client = daemon.client().await.unwrap();

    assert_eq!(ids(&search(&client, "zebraoutput").await), []);
    assert_eq!(search_in(&client, id(1), "zebraoutput").await.hits, []);
    assert_eq!(ids(&search(&client, "make").await), [id(1)], "the command is found");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn query_never_finds_a_heredoc_body() {
    let turn = Turn {
        prompt: "write the env file".to_owned(),
        command: Some("cat > .env <<'EOF'\nAPI_KEY: zebrakey123\nEOF\necho written".to_owned()),
        ..Turn::default()
    };
    let daemon = seeded(conversations(&[vec![turn]]), |b| b).await;
    let client = daemon.client().await.unwrap();

    assert_eq!(ids(&search(&client, "zebrakey123").await), []);
    assert_eq!(search_in(&client, id(1), "zebrakey123").await.hits, []);
    let preview = search_in(&client, id(1), "written").await;
    let [hit] = preview.hits.as_slice() else { panic!("{preview:?}") };
    assert_eq!(hit.kind, SearchKind::Command);
    assert!(preview_texts(&preview).iter().all(|text| !text.contains("zebrakey")));
    daemon.stop().await.unwrap();
}

/// A key of a form that the redaction recognises.
const KEY: &str = "sk-ant-AbCdEfGhIjKlMnOpQrStUvWxYz0123";

#[tokio::test]
async fn rows_show_the_redacted_title() {
    let prompt = format!("deploy with {KEY} to staging");
    let daemon = seeded(conversations(&[vec![Turn::prompt(&prompt)]]), |b| b).await;
    let client = daemon.client().await.unwrap();

    let found = search(&client, "staging").await;

    let row = &found.rows[0];
    let title = row.conversation.summary.title.clone().unwrap();
    assert_eq!(title, "deploy with [***] to staging");
    let line = SearchLine { text: title, ranges: row.title_ranges.clone() };
    assert_eq!(marked(&line), ["staging"]);
    let preview = search_in(&client, id(1), "staging").await;
    assert_eq!(preview.title.unwrap().text, "deploy with [***] to staging");
    assert!(list_texts(&found).iter().all(|text| !text.contains("AbCdEfGh")));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn redacted_marker_is_not_found() {
    let prompt = format!("deploy with {KEY} to staging");
    let daemon = seeded(conversations(&[vec![Turn::prompt(&prompt)]]), |b| b).await;
    let client = daemon.client().await.unwrap();

    assert_eq!(ids(&search(&client, "redacted").await), []);
    assert_eq!(search_in(&client, id(1), "redacted").await.hits, []);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn long_query_is_invalid() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let long = "a".repeat(257);
    let many = vec!["word"; 17].join(" ");

    for (query, message) in [
        (&long, "the query is longer than 256 characters"),
        (&many, "the query has more than 16 words"),
    ] {
        let list = client
            .call::<ConversationsSearchResult>(Method::ConversationsSearch(list_params(query)))
            .await;
        assert_eq!(refused(list), (ErrorCode::Invalid, message.to_owned()));
        let preview =
            ConversationSearch { conversation_id: id(1), query: query.clone(), limit: None };
        let preview =
            client.call::<ConversationSearchResult>(Method::ConversationSearch(preview)).await;
        assert_eq!(refused(preview), (ErrorCode::Invalid, message.to_owned()));
    }
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn preview_of_a_missing_conversation_is_not_found() {
    let daemon = TestDaemon::start().await.unwrap();
    let client = daemon.client().await.unwrap();
    let params = ConversationSearch { conversation_id: id(9), query: String::new(), limit: None };

    let answer = client.call::<ConversationSearchResult>(Method::ConversationSearch(params)).await;

    assert_eq!(refused(answer).0, ErrorCode::NotFound);
    daemon.stop().await.unwrap();
}

/// The list of `,resume` of `tty` in `pwd`, and the search with an empty query of the
/// same terminal and place.
async fn both_lists(
    daemon: &TestDaemon,
    tty: &str,
    pwd: &Path,
) -> (ConversationsRecentResult, ConversationsSearchResult) {
    let client = daemon.client().await.unwrap();
    let recent = client
        .call(Method::ConversationsRecent(ConversationsRecent {
            tty: Some(tty.to_owned()),
            pwd: Some(pwd.to_path_buf()),
            limit: Some(50),
        }))
        .await
        .unwrap();
    let searched = client
        .call(Method::ConversationsSearch(ConversationsSearch {
            query: "  ".to_owned(),
            tty: Some(tty.to_owned()),
            pwd: Some(pwd.to_path_buf()),
            limit: Some(50),
            stats: false,
        }))
        .await
        .unwrap();
    (recent, searched)
}

#[tokio::test]
async fn empty_query_has_the_order_of_the_recent_list() {
    let place = Place::start().await;
    place.at(1, &place.app, "pts-1", 100).await;
    place.at(2, &place.home, "pts-1", 100).await;
    place.at(3, &place.app, "pts-2", 200).await;
    place.at(4, &place.lib, "pts-2", 200).await;
    place.at(5, &place.app, "pts-2", 200).await;

    let (recent, searched) = both_lists(&place.daemon, "pts-1", &place.app.join("src")).await;

    assert_eq!((&searched.scope, &searched.current), (&recent.scope, &recent.current));
    assert_eq!(searched.rows.len(), recent.conversations.len());
    assert_eq!(searched.rows.len(), 4);
    for (row, listed) in searched.rows.iter().zip(&recent.conversations) {
        assert_eq!(row.conversation, *listed, "the same row, with the same title here");
        assert!(listed.summary.id.to_string().starts_with(&row.short_id));
        assert_eq!((row.hit.as_ref(), row.title_ranges.len()), (None, 0));
    }
    assert_eq!(searched.rows[0].conversation.project.as_deref(), Some("app"));
    place.stop().await;
}

/// The words before the key in [`cut_prompt`]: 57 characters, so the cut of a title
/// at 80 characters keeps 13 characters of the key's body, too few for the redaction.
const BEFORE_KEY: &str = "deploy the app to the big cluster today and use the key: ";

/// A prompt whose title the projection cuts in the middle of a key.
fn cut_prompt() -> String {
    format!("{BEFORE_KEY}{KEY} please")
}

#[tokio::test]
async fn empty_query_shows_redacted_titles() {
    assert_eq!(BEFORE_KEY.chars().count(), 57);
    let place = Place::start().await;
    let mut params = Place::prompt(1, &place.app, "pts-1", 100);
    params.text = cut_prompt();
    let sent: PromptSendResult = place.send(params).await;

    // Before the indexer reached it: the projection's title, cut in the key.
    let (recent, searched) = both_lists(&place.daemon, "pts-1", &place.app).await;
    let raw = recent.conversations[0].summary.title.clone().unwrap();
    assert_eq!(raw, format!("{BEFORE_KEY}sk-ant-AbCdEfGhIjKlM..."), "the list as it was");
    let shown = searched.rows[0].conversation.summary.title.clone().unwrap();
    assert_eq!(shown, "deploy the app to the big cluster today and use the key: ...");

    // After: the title of the indexed text of the prompt.
    let client = place.daemon.client().await.unwrap();
    let events = place.daemon.events(&client, sent.conversation_id).await.unwrap();
    indexed(&place.daemon, events.last().unwrap().seq).await;
    let (_, searched) = both_lists(&place.daemon, "pts-1", &place.app).await;
    let shown = searched.rows[0].conversation.summary.title.clone().unwrap();
    assert_eq!(shown, format!("{BEFORE_KEY}[***] please"));
    for text in [raw.as_str(), shown.as_str()] {
        assert!(!text.contains("NoPqRsTu"), "{text}");
    }
    assert!(!shown.contains("AbCdEfGh"), "{shown}");
    place.stop().await;
}

#[tokio::test]
async fn query_does_not_sync_the_projects_file() {
    let place = Place::start().await;
    let sent = place.at(1, &place.app, "pts-1", 100).await;
    let client = place.daemon.client().await.unwrap();
    let events = place.daemon.events(&client, sent.conversation_id).await.unwrap();
    indexed(&place.daemon, events.last().unwrap().seq).await;
    let dirs = place.daemon.dirs().dirs();
    let copy = dirs.runtime().join("projects.tsv");
    let registry = dirs.config().join("projects.toml");
    let before = std::fs::read_to_string(&copy).unwrap();
    // A hand edit of the registry, which the plugin's copy does not hold yet.
    let tool = place.home.join("p/tool");
    let mut text = std::fs::read_to_string(&registry).unwrap();
    text.push_str(&format!(
        "\n[[project]]\nid = \"0192f0c1-7a00-7000-8000-0000000a4403\"\nroot = \"{}\"\nname = \"tool\"\n",
        tool.display()
    ));
    std::fs::write(&registry, text).unwrap();

    let found = search(&client, "prompt").await;

    assert_eq!(ids(&found), [sent.conversation_id]);
    assert_eq!(
        found.rows[0].conversation.project.as_deref(),
        Some("app"),
        "a query names projects"
    );
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), before, "a query reads no place");
    search(&client, "").await;
    assert!(std::fs::read_to_string(&copy).unwrap().contains("tool"), "the empty query does");
    place.stop().await;
}

#[tokio::test]
async fn index_percent_while_backfilling() {
    // 40 prompts, more than one page of the backfill, then 6,000 events with no text.
    let prompts: Vec<Turn> = (0..40).map(|n| Turn::prompt(&format!("prompt {n}"))).collect();
    let mut batches = conversations(&[prompts]);
    let conversation: ConversationId = id(2);
    let turn_id: TurnId = id(9_000_000);
    let call_id: CallId = id(9_000_001);
    let mut filler = turn_batch(conversation, 9_000, true, &Turn::prompt("quiet"));
    filler = filler.event(
        conversation,
        Event::ToolCallStarted {
            turn_id,
            call_id,
            tool: "read_file".to_owned(),
            input: json!({ "path": "/srv/p/a" }),
            freeform: false,
            manual_input: false,
            launch: None,
        },
    );
    for bytes in 0..6_000 {
        filler = filler.event(
            conversation,
            Event::ToolCallOutputUpdated { turn_id, call_id, tail: "x".to_owned(), bytes },
        );
    }
    batches.push(filler);
    let (dirs, newest) = seed(batches).await;
    let daemon = TestDaemon::builder().dirs(dirs).persistent().start().await.unwrap();
    let mut status = daemon.search_index();
    let paused = *status.wait_for(|status| status.sleeping_until.is_some()).await.unwrap();
    assert!(paused.indexed_seq < newest);
    let client = daemon.client().await.unwrap();

    let during = search(&client, "").await;

    let expected = paused.indexed_seq.get() * 100 / newest.get();
    assert_eq!(during.index.map(|index| u64::from(index.percent)), Some(expected));
    assert!(expected < 50, "{expected}");
    indexed(&daemon, newest).await;
    assert_eq!(search(&client, "").await.index, None, "the index is complete");
    assert_eq!(search(&client, "prompt").await.index, None);
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn answer_fits_one_mib() {
    // The longest titles and snippets: 80 and 400 characters of three bytes each.
    let wide = "字".repeat(2_000);
    let turns: Vec<Vec<Turn>> = (0..200)
        .map(|n| vec![Turn::prompt(&format!("{} zebra {n} {wide}", "題".repeat(70)))])
        .collect();
    let daemon = seeded(conversations(&turns), |b| b).await;
    let client = daemon.client().await.unwrap();

    let found = search(&client, "zebra").await;

    assert_eq!(found.rows.len(), 200);
    let size = serde_json::to_vec(&found).unwrap().len();
    assert!(size < 1024 * 1024, "{size} bytes");
    for row in &found.rows {
        let line = &row.hit.as_ref().unwrap().lines[0];
        assert!(line.text.chars().count() <= 400, "{}", line.text.chars().count());
    }
    daemon.stop().await.unwrap();
}

/// The searches that a test holds in the row loop: the hook reports each row on
/// `reached` and waits until the test releases it.
struct Hold {
    reached: async_mpsc::UnboundedReceiver<u32>,
    release: mpsc::Sender<()>,
}

impl Hold {
    fn new() -> (Hold, SearchRowHook) {
        let (reached_tx, reached) = async_mpsc::unbounded_channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let hook = SearchRowHook::new(
            1,
            Arc::new(move |rows| {
                let _ = reached_tx.send(rows);
                // NOTE: a test that ends drops the sender, which ends the wait too.
                let _ = release_rx.lock().unwrap().recv();
            }),
        );
        (Hold { reached, release }, hook)
    }

    /// Waits until a search reached a row.
    async fn reached(&mut self) -> u32 {
        self.reached.recv().await.unwrap()
    }

    /// Lets one held search go on.
    fn release(&self) {
        self.release.send(()).unwrap();
    }
}

/// The searches that started their caps on `clock`: the sleeps of the soft cap.
fn searches_started(clock: &TestClock) -> usize {
    clock.requested_sleeps().iter().filter(|sleep| **sleep == SOFT_CAP).count()
}

/// Waits until a search after the first `before` started its caps, so a test moves
/// the clock only after the sleeps of the caps exist.
async fn search_started(clock: &TestClock, before: usize) {
    while searches_started(clock) <= before {
        clock.wait_for_sleeps(clock.pending_sleeps() + 1).await;
    }
}

/// Lets the daemon's tasks run, so a stop that the clock or a client caused reaches
/// the search before the test lets the search go on.
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

/// A daemon whose store holds three conversations with 20 prompts each that match
/// `zebra`, and the hold of its searches.
async fn held_daemon() -> (TestDaemon, Hold) {
    let (hold, hook) = Hold::new();
    let turns: Vec<Vec<Turn>> = (0..3)
        .map(|c| (0..20).map(|n| Turn::prompt(&format!("zebra {c} {n}"))).collect())
        .collect();
    let daemon = seeded(conversations(&turns), |builder| builder.search_hook(hook)).await;
    (daemon, hold)
}

/// Releases the held searches until `answer` comes; the rows that the searches read
/// after their first.
async fn release_until<T>(hold: &mut Hold, answer: impl Future<Output = T>) -> (T, u32) {
    let mut answer = std::pin::pin!(answer);
    let mut more = 0;
    loop {
        tokio::select! {
            biased;
            value = &mut answer => return (value, more),
            _ = hold.reached() => {
                more += 1;
                settle().await;
                hold.release();
            }
        }
    }
}

#[tokio::test]
async fn time_cap_gives_partial() {
    let (daemon, mut hold) = held_daemon().await;
    let clock = daemon.clock().clone();
    let client = Arc::new(daemon.client().await.unwrap());

    // At the soft cap, the search ranks the rows that it has seen.
    let asking = Arc::clone(&client);
    let pending = tokio::spawn(async move { search(&asking, "zebra").await });
    assert_eq!(hold.reached().await, 1);
    clock.advance(SOFT_CAP);
    settle().await;
    hold.release();
    let (answer, more) = release_until(&mut hold, pending).await;
    let answer = answer.unwrap();
    assert_eq!(answer.partial, Some(SearchPartial::TimeCap));
    assert!(more < 5, "the search went on for {more} rows after its soft cap");
    assert!(!answer.rows.is_empty(), "the rows it saw");

    // At the hard cap, efrd answers with no rows and does not wait for the search.
    let asking = Arc::clone(&client);
    let pending = tokio::spawn(async move { search(&asking, "zebra").await });
    assert_eq!(hold.reached().await, 1);
    clock.advance(HARD_CAP);
    let answer = pending.await.unwrap();
    assert_eq!(answer.partial, Some(SearchPartial::TimeCap));
    assert_eq!(answer.rows, []);
    hold.release();
    // The lane is free again: a search with no rows to read answers.
    assert_eq!(search(&client, "nothingmatches").await.rows, []);
    assert!(hold.reached.try_recv().is_err(), "the stopped search read no other row");
    daemon.stop().await.unwrap();
}

/// Starts a search of `zebra` on each of `clients` and waits until all three hold a
/// lane.
async fn hold_three(
    clients: &[Client],
    hold: &mut Hold,
) -> Vec<ItemStream<ConversationsSearchResult>> {
    let mut streams = Vec::new();
    for client in clients {
        streams
            .push(client.stream(Method::ConversationsSearch(list_params("zebra"))).await.unwrap());
        assert_eq!(hold.reached().await, 1);
    }
    streams
}

async fn three_clients(daemon: &TestDaemon) -> Vec<Client> {
    let mut clients = Vec::new();
    for _ in 0..3 {
        clients.push(daemon.client().await.unwrap());
    }
    clients
}

#[tokio::test]
async fn cancel_frees_the_lane() {
    let (daemon, mut hold) = held_daemon().await;
    let clients = three_clients(&daemon).await;
    let mut streams = hold_three(&clients, &mut hold).await;

    for stream in &mut streams {
        stream.cancel().await.unwrap();
        let end = stream.next().await.unwrap();
        assert_eq!(refused(end).0, ErrorCode::Cancelled);
    }
    for _ in 0..3 {
        hold.release();
    }
    let fresh = daemon.client().await.unwrap();
    let (answer, more) = release_until(&mut hold, search(&fresh, "")).await;

    assert!(answer.partial.is_none());
    assert_eq!(more, 0, "a cancelled search read no other row");
    daemon.stop().await.unwrap();
}

/// A connection that says hello and asks for a search of `zebra`, with no client
/// behind it: dropping it closes the socket with no cancel frame.
async fn raw_search(daemon: &TestDaemon) -> tokio::net::UnixStream {
    use efr_protocol::framing::encode;
    use efr_protocol::{Capabilities, ClientFrame, Hello, PROTOCOL_VERSION, RequestId};
    use tokio::io::AsyncWriteExt as _;

    let mut stream = tokio::net::UnixStream::connect(daemon.socket_path()).await.unwrap();
    let hello = Hello {
        protocol: PROTOCOL_VERSION,
        origin: Origin::Cli,
        client: Some("raw".to_owned()),
        capabilities: Capabilities::default(),
        tty: None,
        pid: None,
        device_id: None,
    };
    let frames = [
        ClientFrame::Request { id: RequestId::new(1), method: Method::Hello(hello) },
        ClientFrame::Request {
            id: RequestId::new(2),
            method: Method::ConversationsSearch(list_params("zebra")),
        },
    ];
    for frame in frames {
        stream.write_all(&encode(&frame).unwrap()).await.unwrap();
    }
    stream
}

#[tokio::test]
async fn closed_client_frees_the_lane() {
    let (daemon, mut hold) = held_daemon().await;
    let mut sockets = Vec::new();
    for _ in 0..3 {
        sockets.push(raw_search(&daemon).await);
        assert_eq!(hold.reached().await, 1);
    }

    drop(sockets);
    settle().await;
    for _ in 0..3 {
        hold.release();
    }
    let fresh = daemon.client().await.unwrap();
    let (answer, more) = release_until(&mut hold, search(&fresh, "")).await;

    assert!(answer.partial.is_none());
    // NOTE: each search has 20 rows; one that efrd did not stop would read them all.
    assert!(more < 6, "the searches of a closed client read {more} more rows");
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn prompt_send_takes_no_search_lane() {
    let (daemon, mut hold) = held_daemon().await;
    let clients = three_clients(&daemon).await;
    let streams = hold_three(&clients, &mut hold).await;

    // Every lane is held: a prompt, its turn and the list of `,resume` still run.
    let shell = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult = shell.call(daemon.prompt(1, "a turn", TTY)).await.unwrap();
    let mut follow = daemon.follow(&shell, sent.conversation_id).await.unwrap();
    efr_test_daemon::events_until(&mut follow, |event| {
        matches!(event, Event::TurnFailed { .. } | Event::TurnCompleted { .. })
    })
    .await
    .unwrap();
    let recent: ConversationsRecentResult =
        shell.call(Method::ConversationsRecent(ConversationsRecent::default())).await.unwrap();
    assert!(!recent.conversations.is_empty());

    // A fourth search waits for a lane, and is busy when none frees in time.
    let fourth = daemon.client().await.unwrap();
    let before = searches_started(daemon.clock());
    let busy = tokio::spawn(async move {
        fourth.call::<ConversationsSearchResult>(Method::ConversationsSearch(list_params(""))).await
    });
    search_started(daemon.clock(), before).await;
    daemon.clock().advance(HARD_CAP);
    assert_eq!(
        refused(busy.await.unwrap()),
        (ErrorCode::Busy, "every search lane is busy".to_owned())
    );
    drop(streams);
    for _ in 0..3 {
        hold.release();
    }
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn search_logs_no_query_text() {
    use std::os::unix::fs::PermissionsExt as _;

    const QUERY: &str = "zebraquery";
    logs();
    let (dirs, newest) =
        seed(conversations(&[vec![Turn::prompt("find the zebraquery here")]])).await;
    let database = dirs.dirs().data().join(efr_store::DATABASE_FILE);
    let daemon = TestDaemon::builder().dirs(dirs).persistent().start().await.unwrap();
    indexed(&daemon, newest).await;
    let client = daemon.client().await.unwrap();

    // A lane that cannot open the database fails the search with a fixed sentence.
    let mode = std::fs::metadata(&database).unwrap().permissions().mode();
    std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o000)).unwrap();
    let failed = client
        .call::<ConversationsSearchResult>(Method::ConversationsSearch(list_params(QUERY)))
        .await;
    std::fs::set_permissions(&database, std::fs::Permissions::from_mode(mode)).unwrap();
    if !rustix::process::geteuid().is_root() {
        let (code, message) = refused(failed);
        assert_eq!(
            (code, message.as_str()),
            (ErrorCode::Internal, "the search failed in the store")
        );
    }
    let found = search(&client, QUERY).await;
    assert_eq!(found.rows.len(), 1);
    search_in(&client, id(1), QUERY).await;
    daemon.stop().await.unwrap();

    let logged = logged();
    assert!(logged.contains("searched the conversations"), "the debug line of a search");
    assert!(logged.contains("searched a conversation"), "the debug line of a preview");
    let lines: Vec<&str> = logged.lines().filter(|line| line.contains(QUERY)).collect();
    assert_eq!(lines, Vec::<&str>::new(), "no log line holds the query");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(6))]

    /// No 8 characters of a secret that the index redaction recognises are in an
    /// answer of either search method, when the search is for those 8 characters:
    /// not in a title, a snippet or a line of the preview. One conversation per form
    /// of secret, with the secret in its prompt, a steer, an answer and a command.
    #[test]
    fn secret_is_not_found_by_text_or_title(seed in any::<u64>()) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(secrets_not_found(seed));
    }
}

async fn secrets_not_found(seed: u64) {
    let mut turns = Vec::new();
    let mut secrets = Vec::new();
    for form in 0..FORMS {
        let (text, secret) = secret_text(form, seed.wrapping_add(form as u64));
        turns.push(vec![Turn {
            prompt: text.clone(),
            steer: Some(text.clone()),
            answer: Some(text.clone()),
            command: Some(text),
            output: None,
        }]);
        secrets.push(secret);
    }
    let daemon = seeded(conversations(&turns), |b| b).await;
    let client = daemon.client().await.unwrap();
    for (index, secret) in secrets.iter().enumerate() {
        let chars: Vec<char> = secret.chars().collect();
        let windows: Vec<String> = chars.windows(8).map(|part| part.iter().collect()).collect();
        let picked = windows.iter().step_by(4).chain(windows.last());
        for part in picked {
            let found = search(&client, part).await;
            for text in list_texts(&found) {
                assert!(!text.contains(part.as_str()), "{part:?} of {secret:?} is in {text:?}");
            }
            let preview = search_in(&client, id(index as u64 + 1), part).await;
            for text in preview_texts(&preview) {
                assert!(!text.contains(part.as_str()), "{part:?} of {secret:?} is in {text:?}");
            }
        }
    }
    daemon.stop().await.unwrap();
}
