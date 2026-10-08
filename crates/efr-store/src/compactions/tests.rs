use efr_protocol::{Compaction, ConversationId, Event, Seq};
use pretty_assertions::assert_eq;

use super::*;
use crate::testing::{self, TestClock, on_writer};
use crate::{Batch, WriterHandle};

fn compaction(event: Event) -> Compaction {
    match event {
        Event::ConversationCompacted(compaction) => compaction,
        other => panic!("not a compaction: {other:?}"),
    }
}

async fn read(writer: &WriterHandle, id: ConversationId) -> LatestCompactions {
    on_writer(writer, move |conn| latest(conn, id)).await.unwrap()
}

#[tokio::test]
async fn the_newest_summary_and_the_newest_compaction_come_back_with_their_seqs() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let (one, two) = (testing::conversation(1), testing::conversation(2));
    writer
        .append(Batch::new().event(one, testing::created(None)).event(two, testing::created(None)))
        .await
        .unwrap();
    assert_eq!(read(&writer, one).await, LatestCompactions::default());

    writer
        .append(
            Batch::new()
                .event(one, testing::compacted(1, 1, None, true))
                .event(one, testing::compacted(2, 2, Some(3), true))
                .event(one, testing::compacted(3, 3, None, false))
                .event(two, testing::compacted(4, 9, None, true)),
        )
        .await
        .unwrap();

    let latest = read(&writer, one).await;
    assert_eq!(
        latest.summary,
        Some(StoredCompaction {
            seq: Seq::new(4),
            compaction: compaction(testing::compacted(2, 2, Some(3), true)),
        })
    );
    assert_eq!(
        latest.newest,
        Some(StoredCompaction {
            seq: Seq::new(5),
            compaction: compaction(testing::compacted(3, 3, None, false)),
        })
    );
    let all = on_writer(&writer, move |conn| of_conversation(conn, one)).await.unwrap();
    assert_eq!(all.iter().map(|stored| stored.seq.get()).collect::<Vec<_>>(), [3, 4, 5]);
    assert_eq!(read(&writer, two).await.summary.map(|stored| stored.seq), Some(Seq::new(6)));
}

#[tokio::test]
async fn a_rebuild_reproduces_the_compactions() {
    let (writer, _thread) = testing::memory_writer(TestClock::new());
    let id = testing::conversation(1);
    writer
        .append(
            Batch::new()
                .event(id, testing::created(None))
                .event(id, testing::compacted(1, 1, None, true))
                .event(id, testing::compacted(2, 2, None, false)),
        )
        .await
        .unwrap();
    let before = on_writer(&writer, |conn| Ok(testing::dump(conn, "compactions"))).await.unwrap();

    writer.rebuild_projections().await.unwrap();

    let after = on_writer(&writer, |conn| Ok(testing::dump(conn, "compactions"))).await.unwrap();
    assert_eq!(after, before);
    assert_eq!(after.len(), 2);
}
