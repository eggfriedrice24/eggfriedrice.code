//! Drafts on `conversation.subscribe` over a `TestDaemon`: they reach only a
//! subscriber that asked for them, after the events that the turn recorded before
//! them, within 35 ms at the 95th percentile; the log is the same with and without
//! them; and a slow subscriber loses drafts but keeps its subscription.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use efr_protocol::framing::{self, Decoder};
use efr_protocol::{
    Capabilities, ClientFrame, ConversationId, ConversationSubscribe, ConversationSubscribeItem,
    DraftPart, Event, Hello, Method, Origin, PROTOCOL_VERSION, PromptSendResult, RequestId, Seq,
    ServerFrame,
};
use efr_provider::{
    Provider, ProviderError, ProviderEvent, ProviderId, ProviderStream, Request, StopReason,
};
use efr_stdx::time::{Clock as _, SystemClock};
use efr_test_daemon::{Client, ItemStream, TTY, TestDaemon};
use futures::StreamExt as _;
use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::UnixStream;
use tokio::sync::Notify;

/// When each delta left the model: the end of the text it completes, in bytes, and the
/// time.
type Sent = Arc<Mutex<Vec<(u64, Instant)>>>;

/// A model that waits for the test (`go`), then sends `deltas` of text, each after
/// `gap` of real time, and notes when each one left. With `hold_end`, it waits for the
/// test again (`end`) before it ends the answer.
#[derive(Debug)]
struct Streamer {
    id: ProviderId,
    deltas: Vec<String>,
    gap: Duration,
    go: Arc<Notify>,
    hold_end: bool,
    end: Arc<Notify>,
    sent: Sent,
    /// Notified when the turn calls the model.
    called: Arc<Notify>,
}

impl Streamer {
    fn new(deltas: Vec<String>, gap: Duration) -> Arc<Self> {
        Arc::new(Streamer {
            id: ProviderId::new("test").unwrap(),
            deltas,
            gap,
            go: Arc::new(Notify::new()),
            hold_end: false,
            end: Arc::new(Notify::new()),
            sent: Arc::default(),
            called: Arc::new(Notify::new()),
        })
    }

    /// A model that waits for the test before it ends the answer, so the drafts of the
    /// text reach the subscriber before the completion makes them old.
    fn held(deltas: Vec<String>) -> Arc<Self> {
        let mut model = Arc::into_inner(Streamer::new(deltas, Duration::ZERO)).unwrap();
        model.hold_end = true;
        Arc::new(model)
    }

    fn text(&self) -> String {
        self.deltas.concat()
    }
}

#[async_trait]
impl Provider for Streamer {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn stream(&self, _request: Request) -> Result<ProviderStream, ProviderError> {
        self.called.notify_one();
        let go = Arc::clone(&self.go);
        let end_gate = self.hold_end.then(|| Arc::clone(&self.end));
        let gap = self.gap;
        let sent = Arc::clone(&self.sent);
        let deltas = self.deltas.clone();
        let events = futures::stream::unfold((0, 0_u64), move |(at, end)| {
            let go = Arc::clone(&go);
            let end_gate = end_gate.clone();
            let sent = Arc::clone(&sent);
            let deltas = deltas.clone();
            async move {
                if at == 0 {
                    go.notified().await;
                } else if !gap.is_zero() {
                    SystemClock.sleep(gap).await;
                }
                let Some(delta) = deltas.get(at) else {
                    if at == deltas.len() {
                        if let Some(gate) = end_gate {
                            gate.notified().await;
                        }
                        let done = ProviderEvent::Done {
                            stop_reason: StopReason::EndTurn,
                            provider_raw: None,
                        };
                        return Some((Ok(done), (at + 1, end)));
                    }
                    return None;
                };
                let end = end + delta.len() as u64;
                sent.lock().unwrap_or_else(PoisonError::into_inner).push((end, Instant::now()));
                Some((Ok(ProviderEvent::TextDelta { text: delta.clone() }), (at + 1, end)))
            }
        });
        Ok(Box::pin(events))
    }
}

/// `count` words of text.
fn words(count: usize) -> Vec<String> {
    (0..count).map(|n| format!("word{n} ")).collect()
}

/// Subscribes `client` to `conversation_id` from the start, with or without drafts, and
/// waits for the start, so every later draft reaches the stream.
async fn subscribe(
    client: &Client,
    conversation_id: ConversationId,
    drafts: bool,
) -> (ItemStream<ConversationSubscribeItem>, Seq) {
    let params =
        ConversationSubscribe { conversation_id, after_seq: None, answers_input: false, drafts };
    let mut stream = client.stream(Method::ConversationSubscribe(params)).await.unwrap();
    let first = stream.next().await.unwrap().unwrap();
    let ConversationSubscribeItem::Snapshot(snapshot) = first else {
        panic!("a subscription without after_seq starts with a snapshot: {first:?}");
    };
    (stream, snapshot.hwm)
}

/// What one subscription saw so far.
#[derive(Debug, Default)]
struct Seen {
    /// The kind of each event, with its sequence number.
    kinds: Vec<(Seq, String)>,
    /// Each text draft: its offset and its delta.
    texts: Vec<(u64, String)>,
    /// When the client held the text up to each end, from drafts and from events.
    shown: Vec<(u64, Instant)>,
    drafts: usize,
    updates: usize,
    /// The last event, or the mark of the start.
    last: Seq,
    /// The last event that ends what a draft shows.
    boundary: Seq,
    done: bool,
}

impl Seen {
    fn after(hwm: Seq) -> Self {
        Seen { last: hwm, ..Seen::default() }
    }

    /// The kinds of the events after `after`.
    fn kinds_after(&self, after: Seq) -> Vec<&str> {
        self.kinds.iter().filter(|(seq, _)| *seq > after).map(|(_, kind)| kind.as_str()).collect()
    }

    /// Reads `stream` until `enough` holds or the turn completed. Checks that every
    /// event at or below the `after_seq` of a draft came before it, and that no draft
    /// came after an event that ends what it shows.
    async fn read(
        &mut self,
        stream: &mut ItemStream<ConversationSubscribeItem>,
        enough: impl Fn(&Seen) -> bool,
    ) {
        while !self.done && !enough(self) {
            match stream.next().await.unwrap().unwrap() {
                ConversationSubscribeItem::Event(envelope) => {
                    self.last = envelope.seq;
                    self.kinds.push((envelope.seq, envelope.event.kind().to_owned()));
                    match envelope.event {
                        Event::AssistantMessageUpdated { offset, delta, .. } => {
                            self.updates += 1;
                            self.shown.push((offset + delta.len() as u64, Instant::now()));
                        }
                        Event::AssistantMessageCompleted { text, .. } => {
                            self.shown.push((text.len() as u64, Instant::now()));
                            self.boundary = envelope.seq;
                        }
                        Event::TurnCompleted { .. } => self.done = true,
                        _ => {}
                    }
                }
                ConversationSubscribeItem::Draft(draft) => {
                    let (last, boundary) = (self.last, self.boundary);
                    assert!(draft.after_seq <= last, "{draft:?} came before event {last}");
                    assert!(draft.after_seq >= boundary, "{draft:?} is older than {boundary}");
                    self.drafts += 1;
                    if let DraftPart::Text { index, offset, delta } = draft.draft {
                        assert_eq!(index, 0);
                        self.shown.push((offset + delta.len() as u64, Instant::now()));
                        self.texts.push((offset, delta));
                    }
                }
                other => panic!("only events and drafts after the start: {other:?}"),
            }
        }
    }
}

/// Reads `stream` up to the end of the turn.
async fn read_turn(stream: &mut ItemStream<ConversationSubscribeItem>, hwm: Seq) -> Seen {
    let mut seen = Seen::after(hwm);
    seen.read(stream, |_| false).await;
    seen
}

/// Starts a daemon whose model is `model`, with `draft_interval_ms`, and sends one
/// prompt; returns the daemon, its client and the conversation.
async fn prompted(
    model: &Arc<Streamer>,
    draft_interval_ms: u64,
) -> (TestDaemon, Client, ConversationId) {
    let daemon = TestDaemon::builder()
        .custom_provider(Arc::clone(model) as Arc<dyn Provider>)
        .config(|config| config.conversation.draft_interval_ms = draft_interval_ms)
        .start()
        .await
        .unwrap();
    let client = daemon.client_for_tty(TTY).await.unwrap();
    let sent: PromptSendResult = client.call(daemon.prompt(1, "talk", TTY)).await.unwrap();
    (daemon, client, sent.conversation_id)
}

#[tokio::test]
async fn drafts_reach_a_subscriber_that_asked_and_no_other() {
    let model = Streamer::held(words(40));
    let (daemon, client, conversation_id) = prompted(&model, 0).await;
    let (mut with_stream, with_hwm) = subscribe(&client, conversation_id, true).await;
    let (mut without_stream, without_hwm) = subscribe(&client, conversation_id, false).await;
    model.go.notify_one();

    let mut with = Seen::after(with_hwm);
    // NOTE: the status of the turn may come first; the text drafts are the point here.
    with.read(&mut with_stream, |seen| !seen.texts.is_empty()).await;
    model.end.notify_one();
    with.read(&mut with_stream, |_| false).await;
    let without = read_turn(&mut without_stream, without_hwm).await;

    assert_eq!(without.drafts, 0, "no drafts without the parameter");
    assert!(!with.texts.is_empty(), "drafts with the parameter");
    // NOTE: the turn records its start while the test subscribes, so the start can come
    // between the two subscriptions. Each one gets the events after its own start, so
    // the two get the same events after the later start.
    let after = with_hwm.max(without_hwm);
    assert_eq!(with.kinds_after(after), without.kinds_after(after), "both get the same events");
    // Each draft is the text at its offset, and the offsets only grow.
    let whole = model.text();
    let mut end = 0;
    for (offset, delta) in &with.texts {
        let at = usize::try_from(*offset).unwrap();
        assert!(at >= end, "{:?}", with.texts);
        assert_eq!(whole.get(at..at + delta.len()), Some(delta.as_str()));
        end = at + delta.len();
    }
    drop((with_stream, without_stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn a_subscriber_that_attaches_during_a_model_call_gets_the_context_at_once() {
    let model = Streamer::held(words(4));
    let (daemon, client, conversation_id) = prompted(&model, 0).await;
    model.called.notified().await;

    let (mut stream, _) = subscribe(&client, conversation_id, true).await;

    let first = stream.next().await.unwrap().unwrap();
    let ConversationSubscribeItem::Draft(draft) = first else {
        panic!("the status of the running turn comes first: {first:?}");
    };
    assert!(matches!(draft.draft, DraftPart::Context(_)), "{draft:?}");
    model.go.notify_one();
    model.end.notify_one();
    drop((stream, client));
    daemon.stop().await.unwrap();
}

#[tokio::test]
async fn the_store_holds_the_same_events_with_and_without_drafts() {
    let mut logs = Vec::new();
    for drafts in [false, true] {
        let model = Streamer::held(words(40));
        let (daemon, client, conversation_id) = prompted(&model, 0).await;
        let (mut stream, hwm) = subscribe(&client, conversation_id, drafts).await;
        model.go.notify_one();
        let mut seen = Seen::after(hwm);
        seen.read(&mut stream, |seen| seen.updates > 0 && (seen.drafts > 0) == drafts).await;
        model.end.notify_one();
        seen.read(&mut stream, |_| false).await;
        assert_eq!(seen.drafts > 0, drafts);
        let log: Vec<Event> = daemon
            .events(&client, conversation_id)
            .await
            .unwrap()
            .into_iter()
            .map(|envelope| envelope.event)
            .collect();
        let kinds: Vec<&str> = log.iter().map(Event::kind).collect();
        assert!(!kinds.iter().any(|kind| kind.contains("draft")), "{kinds:?}");
        logs.push(kinds.iter().map(|kind| (*kind).to_owned()).collect::<Vec<_>>());
        drop((stream, client));
        daemon.stop().await.unwrap();
    }
    assert_eq!(logs[0], logs[1], "drafts never reach the store");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drafts_reach_the_client_within_35_ms_at_the_95th_percentile() {
    // NOTE: the daemon runs on a test clock; here it follows real time, so the draft
    // interval of 16 ms and the wait of the frames are real.
    let model = Streamer::new(words(200), Duration::from_millis(5));
    let (daemon, client, conversation_id) = prompted(&model, 16).await;
    let clock = daemon.clock().clone();
    let follower = tokio::spawn(async move {
        let mut last = Instant::now();
        loop {
            SystemClock.sleep(Duration::from_millis(1)).await;
            let now = Instant::now();
            clock.advance(now - last);
            last = now;
        }
    });
    let (mut stream, hwm) = subscribe(&client, conversation_id, true).await;
    model.go.notify_one();
    let seen = read_turn(&mut stream, hwm).await;
    follower.abort();

    let sent = model.sent.lock().unwrap_or_else(PoisonError::into_inner).clone();
    // The delay of a delta: from when it left the model to when the client first held
    // the text up to its end, from a draft or from an event.
    let mut delays: Vec<Duration> = sent
        .iter()
        .map(|(end, left)| {
            let (_, arrived) = seen
                .shown
                .iter()
                .find(|(shown, _)| shown >= end)
                .expect("every delta reaches the client");
            arrived.saturating_duration_since(*left)
        })
        .collect();
    delays.sort();
    let p95 = delays[delays.len() * 95 / 100];
    assert!(
        p95 < Duration::from_millis(35),
        "p95 {p95:?}, max {:?}, {} text drafts for {} deltas",
        delays.last(),
        seen.texts.len(),
        sent.len()
    );
    assert!(seen.texts.len() > sent.len() / 8, "the drafts carried the text, not the events");
    drop((stream, client));
    daemon.stop().await.unwrap();
}

/// Reads frames from `socket` until `done` accepts one; returns every frame read.
async fn read_frames(
    socket: &mut UnixStream,
    decoder: &mut Decoder,
    mut done: impl FnMut(&ServerFrame) -> bool,
) -> Vec<ServerFrame> {
    let mut frames = Vec::new();
    let mut buffer = vec![0; 256 * 1024];
    loop {
        let read = socket.read(&mut buffer).await.unwrap();
        assert!(read > 0, "the daemon closed the connection");
        for payload in decoder.push(&buffer[..read]).unwrap() {
            let frame = ServerFrame::from_json(&payload).unwrap();
            let last = done(&frame);
            frames.push(frame);
            if last {
                return frames;
            }
        }
    }
}

#[tokio::test]
async fn a_slow_subscriber_loses_drafts_and_keeps_its_subscription() {
    // Big deltas fill the socket and the queues while the subscriber does not read.
    let deltas: Vec<String> = (0..600).map(|n| format!("{n:04}{}", "x".repeat(4092))).collect();
    let model = Streamer::new(deltas, Duration::ZERO);
    let (daemon, client, conversation_id) = prompted(&model, 0).await;

    let mut socket = UnixStream::connect(daemon.socket_path()).await.unwrap();
    let hello = Method::Hello(Hello {
        protocol: PROTOCOL_VERSION,
        origin: Origin::Cli,
        client: Some("slow".to_owned()),
        capabilities: Capabilities::default(),
        tty: None,
        pid: None,
        device_id: None,
    });
    let subscribe = Method::ConversationSubscribe(ConversationSubscribe {
        conversation_id,
        after_seq: None,
        answers_input: false,
        drafts: true,
    });
    let (hello_id, subscribe_id) = (RequestId::new(1), RequestId::new(2));
    for frame in [
        ClientFrame::Request { id: hello_id, method: hello },
        ClientFrame::Request { id: subscribe_id, method: subscribe },
    ] {
        socket.write_all(&framing::encode(&frame).unwrap()).await.unwrap();
    }
    let mut decoder = Decoder::new();
    // The start of the subscription: from here on the drafts reach it.
    read_frames(
        &mut socket,
        &mut decoder,
        |frame| matches!(frame, ServerFrame::Item { id, .. } if *id == subscribe_id),
    )
    .await;

    // The subscriber stops reading while the whole turn runs.
    let (mut watcher, hwm) = subscribe_quietly(&client, conversation_id).await;
    model.go.notify_one();
    read_turn(&mut watcher, hwm).await;

    let frames = read_frames(&mut socket, &mut decoder, |frame| match frame {
        ServerFrame::Item { item, .. } => {
            serde_json::from_value::<ConversationSubscribeItem>(item.clone()).is_ok_and(|item| {
                matches!(item, ConversationSubscribeItem::Event(ref envelope)
                    if matches!(envelope.event, Event::TurnCompleted { .. }))
            })
        }
        ServerFrame::Error(error) => error.id == Some(subscribe_id),
        ServerFrame::End { id } => *id == subscribe_id,
        _ => false,
    })
    .await;
    let mut drafts = 0;
    let mut completed = None;
    for frame in &frames {
        match frame {
            ServerFrame::Item { id, item } if *id == subscribe_id => {
                match serde_json::from_value::<ConversationSubscribeItem>(item.clone()).unwrap() {
                    ConversationSubscribeItem::Draft(_) => drafts += 1,
                    ConversationSubscribeItem::Event(envelope) => {
                        if let Event::AssistantMessageCompleted { text, .. } = envelope.event {
                            completed = Some(text);
                        }
                    }
                    other => panic!("only events and drafts after the start: {other:?}"),
                }
            }
            ServerFrame::Error(error) if error.id == Some(subscribe_id) => {
                panic!("the subscription stays open: {error:?}")
            }
            ServerFrame::End { id } if *id == subscribe_id => {
                panic!("the subscription stays open")
            }
            _ => {}
        }
    }
    assert!(drafts < model.deltas.len(), "the full queue dropped drafts: {drafts}");
    assert_eq!(completed.as_deref(), Some(model.text().as_str()), "every event arrived");
    drop((socket, watcher, client));
    daemon.stop().await.unwrap();
}

/// A subscription without drafts, to wait for the end of the turn.
async fn subscribe_quietly(
    client: &Client,
    conversation_id: ConversationId,
) -> (ItemStream<ConversationSubscribeItem>, Seq) {
    subscribe(client, conversation_id, false).await
}
