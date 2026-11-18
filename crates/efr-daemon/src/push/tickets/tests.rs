use pretty_assertions::assert_eq;

use super::*;

fn at(minutes: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + minutes * 60).unwrap()
}

fn id(n: usize) -> TicketId {
    TicketId::new(&format!("ticket-{n}")).unwrap()
}

fn device() -> DeviceId {
    DeviceId::from_uuid(uuid::Uuid::from_u128(1))
}

fn token() -> SecretText {
    SecretText::new("ExponentPushToken[a]")
}

fn ids(tickets: &[OpenTicket]) -> Vec<String> {
    tickets.iter().map(|ticket| ticket.id.as_str().to_owned()).collect()
}

#[test]
fn a_ticket_is_due_15_minutes_after_its_send_and_none_waits_without_one() {
    let mut open = OpenTickets::default();
    assert_eq!(open.next_due(), None, "no timer runs");
    open.add(id(1), device(), token(), at(0));
    open.add(id(2), device(), token(), at(5));

    assert_eq!(open.next_due(), Some(at(15)));
    assert_eq!(open.take_due(at(14)), []);
    assert_eq!(ids(&open.take_due(at(15))), ["ticket-1"]);
    assert_eq!(open.next_due(), Some(at(20)));
    assert_eq!(ids(&open.take_due(at(30))), ["ticket-2"]);
    assert_eq!(open.next_due(), None);
}

#[test]
fn a_ticket_with_no_receipt_is_asked_once_more_and_then_goes() {
    let mut open = OpenTickets::default();
    open.add(id(1), device(), token(), at(0));

    let first = open.take_due(at(15)).pop().unwrap();
    open.again(first, at(15));
    assert_eq!(open.next_due(), Some(at(30)));
    let second = open.take_due(at(30)).pop().unwrap();
    assert!(second.asked);
    open.again(second, at(30));
    assert_eq!(open.next_due(), None, "the second miss drops it");
}

#[test]
fn at_most_1000_tickets_wait_and_the_oldest_goes_first() {
    let mut open = OpenTickets::default();
    for n in 0..=MAX_RECEIPT_IDS {
        open.add(id(n), device(), token(), at(0));
    }

    let due = open.take_due(at(15));
    assert_eq!(due.len(), MAX_RECEIPT_IDS);
    assert_eq!(due[0].id, id(1), "ticket 0 went");
}

#[test]
fn a_ticket_asked_again_keeps_the_order_of_the_due_times() {
    let mut open = OpenTickets::default();
    open.add(id(1), device(), token(), at(0));
    let first = open.take_due(at(15)).pop().unwrap();
    open.add(id(2), device(), token(), at(10));
    open.again(first, at(16));

    assert_eq!(ids(&open.take_due(at(25))), ["ticket-2"]);
    assert_eq!(ids(&open.take_due(at(31))), ["ticket-1"]);
}
