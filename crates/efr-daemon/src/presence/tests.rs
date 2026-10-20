use std::sync::Arc;
use std::time::Duration;

use efr_config::Settings;
use efr_conversation::PresenceReader as _;
use efr_protocol::{ConversationId, Event, Origin, Presence, PresenceSource};
use efr_stdx::id::uuid_v7;
use efr_store::Batch;
use efr_test_support::{TestClock, TestRng, TestStore, Wait};
use pretty_assertions::assert_eq;
use tokio::sync::watch;

use super::{PresenceTable, batch};

struct Fixture {
    clock: TestClock,
    store: TestStore,
    table: PresenceTable,
    settings: watch::Sender<Arc<Settings>>,
}

impl Fixture {
    async fn new() -> Fixture {
        let clock = TestClock::new();
        let store = TestStore::open(clock.shared()).await.unwrap();
        let (settings, receiver) = watch::channel(Arc::new(Settings::default()));
        let table = PresenceTable::new(receiver, clock.shared(), store.writer().clone());
        Fixture { clock, store, table, settings }
    }

    /// A conversation that the log knows.
    async fn conversation(&self, seed: u64) -> ConversationId {
        let id = ConversationId::from_uuid(uuid_v7(&self.clock, &TestRng::new(seed)));
        let created = Event::ConversationCreated { origin: Origin::Shell, tty: None, route: None };
        self.store.writer().append(Batch::new().event(id, created)).await.unwrap();
        id
    }

    /// The presence changes in the log, in order.
    async fn changes(&self) -> Vec<(ConversationId, Presence, PresenceSource)> {
        self.store
            .events()
            .await
            .unwrap()
            .into_iter()
            .filter_map(|envelope| match envelope.event {
                Event::PresenceChanged { conversation_id, presence, source } => {
                    Some((conversation_id, presence, source))
                }
                _ => None,
            })
            .collect()
    }
}

#[tokio::test]
async fn idle_minutes_make_a_conversation_away_and_the_first_look_records_it() {
    let fixture = Fixture::new().await;
    let id = fixture.conversation(1).await;
    fixture.table.input(id).await;

    fixture.clock.advance(Duration::from_secs(9 * 60));
    assert_eq!(fixture.table.presence(id), Presence::Present, "9 minutes is not idle");
    fixture.clock.advance(Duration::from_secs(60));
    assert_eq!(fixture.table.presence(id), Presence::Away, "10 minutes is");
    assert_eq!(fixture.table.presence(id), Presence::Away);

    let away = vec![(id, Presence::Away, PresenceSource::Timeout)];
    Wait::new("the recorded timeout")
        .until_some_async(async || (fixture.changes().await == away).then_some(()))
        .await
        .unwrap();
    fixture.table.input(id).await;
    assert_eq!(fixture.table.presence(id), Presence::Present);
    assert_eq!(
        fixture.changes().await,
        [
            (id, Presence::Away, PresenceSource::Timeout),
            (id, Presence::Present, PresenceSource::Input)
        ],
        "one event per change, never one per look"
    );
}

#[tokio::test]
async fn input_after_an_unseen_idle_state_records_it_before_the_return() {
    let fixture = Fixture::new().await;
    let id = fixture.conversation(1).await;
    fixture.table.input(id).await;
    fixture.clock.advance(Duration::from_secs(5 * 60));
    fixture.table.input(id).await;
    assert_eq!(fixture.changes().await, [], "input within the limit changes nothing");

    fixture.clock.advance(Duration::from_secs(11 * 60));
    fixture.table.input(id).await;

    assert_eq!(
        fixture.changes().await,
        [
            (id, Presence::Away, PresenceSource::Timeout),
            (id, Presence::Present, PresenceSource::Input)
        ]
    );
}

#[tokio::test]
async fn a_touch_marks_input_now_and_leaves_the_record_to_the_caller() {
    let fixture = Fixture::new().await;
    let id = fixture.conversation(1).await;
    fixture.table.input(id).await;
    fixture.clock.advance(Duration::from_secs(30 * 60));

    let changes = fixture.table.touch(id);
    assert_eq!(fixture.table.presence(id), Presence::Present, "present from the touch on");
    assert_eq!(fixture.changes().await, [], "nothing recorded yet");
    fixture.table.record(changes).await;

    assert_eq!(
        fixture.changes().await,
        [
            (id, Presence::Away, PresenceSource::Timeout),
            (id, Presence::Present, PresenceSource::Input)
        ]
    );
}

#[tokio::test]
async fn the_limit_comes_from_the_latest_settings() {
    let fixture = Fixture::new().await;
    let id = fixture.conversation(1).await;
    fixture.table.input(id).await;
    let mut settings = Settings::default();
    settings.presence.away_after_minutes = 2;
    fixture.settings.send_replace(Arc::new(settings));

    fixture.clock.advance(Duration::from_secs(3 * 60));

    assert_eq!(fixture.table.presence(id), Presence::Away);
}

#[tokio::test]
async fn the_user_sets_away_until_the_next_input() {
    let fixture = Fixture::new().await;
    let id = fixture.conversation(1).await;
    fixture.table.input(id).await;

    let changes = fixture.table.set(&[id], Presence::Away);
    fixture.store.writer().append(batch(changes).unwrap()).await.unwrap();
    assert_eq!(fixture.table.presence(id), Presence::Away);
    assert!(fixture.table.set(&[id], Presence::Away).is_empty(), "away twice changes nothing");
    assert!(fixture.table.present().is_empty());
    fixture.table.input(id).await;

    assert_eq!(fixture.table.presence(id), Presence::Present);
    assert_eq!(
        fixture.changes().await,
        [
            (id, Presence::Away, PresenceSource::User),
            (id, Presence::Present, PresenceSource::Input)
        ]
    );
}

#[tokio::test]
async fn an_unknown_conversation_counts_as_present_from_the_first_look() {
    let fixture = Fixture::new().await;
    let id = fixture.conversation(1).await;
    let other = fixture.conversation(2).await;

    assert_eq!(fixture.table.touch(other), []);
    assert_eq!(fixture.table.known(), [], "an unknown id never enters through a touch");
    assert_eq!(fixture.table.presence(id), Presence::Present);
    assert_eq!(fixture.table.known(), [id]);
    fixture.clock.advance(Duration::from_secs(10 * 60));
    assert_eq!(fixture.table.presence(id), Presence::Away);
}

/// A conversation counts as followed while a client that can type answers subscribes to
/// it; a client that only reads, or none, does not count, and a table with no
/// connections knows nobody.
#[tokio::test]
async fn followed_true_with_an_answering_client() {
    let fixture = Fixture::new().await;
    let id = fixture.conversation(1).await;
    assert!(!fixture.table.followed(id), "a table with no connections");
    let connections = Arc::new(crate::connections::Connections::default());
    let hello = crate::connections::HelloInfo {
        surface: Origin::Shell,
        tty: Some("/dev/pts/3".to_owned()),
        client: None,
    };
    connections.opened(efr_transport::ConnId::new(1), hello);
    let Fixture { clock, store, table, settings: _settings } = fixture;
    let table = table.with_connections(Arc::clone(&connections));
    assert!(!table.followed(id));
    let reading = connections.subscribe(efr_transport::ConnId::new(1), id, false);
    assert!(!table.followed(id), "a subscription that cannot answer");
    let answering = connections.subscribe(efr_transport::ConnId::new(1), id, true);
    assert!(table.followed(id));
    let other = ConversationId::from_uuid(uuid_v7(&clock, &TestRng::new(9)));
    assert!(!table.followed(other));
    drop(answering);
    assert!(!table.followed(id), "the answering client left");
    drop((reading, store));
}
