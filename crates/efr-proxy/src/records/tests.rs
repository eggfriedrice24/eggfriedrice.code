use pretty_assertions::assert_eq;

use super::*;

fn blocked(port: u16, reason: BlockReason, count: u32) -> Blocked {
    Blocked { host: "a.test".to_owned(), port, reason, count }
}

#[test]
fn a_refusal_counts_in_its_record_or_in_the_rest() {
    let mut records = Records::new(2);
    records.add("a.test", 443, BlockReason::NotAllowed);
    records.add("a.test", 443, BlockReason::NotAllowed);
    records.add("a.test", 8443, BlockReason::NotAllowed);
    records.add("a.test", 443, BlockReason::SniMismatch);
    records.add("b.test", 443, BlockReason::NotAllowed);
    records.add("a.test", 443, BlockReason::NotAllowed);
    let (entries, more) = records.snapshot();
    assert_eq!(
        entries,
        vec![blocked(443, BlockReason::NotAllowed, 3), blocked(8443, BlockReason::NotAllowed, 1)]
    );
    assert_eq!(more, 2);
    assert_eq!(records.total(), 6);
}
