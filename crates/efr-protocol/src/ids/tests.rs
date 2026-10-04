use std::error::Error as _;
use std::str::FromStr;

use pretty_assertions::assert_eq;
use uuid::Uuid;

use crate::{ConversationId, ProtocolError, RequestId, Seq, TurnId};

const TEXT: &str = "01928c4e-7a3b-7c1d-8e2f-0123456789ab";

fn conversation() -> ConversationId {
    ConversationId::from_uuid(Uuid::parse_str(TEXT).unwrap())
}

#[test]
fn id_displays_as_the_lowercase_hyphenated_uuid() {
    assert_eq!(conversation().to_string(), TEXT);
}

#[test]
fn id_parses_its_display_form() {
    assert_eq!(ConversationId::from_str(TEXT).unwrap(), conversation());
}

#[test]
fn id_keeps_the_uuid_it_wraps() {
    assert_eq!(conversation().as_uuid(), &Uuid::parse_str(TEXT).unwrap());
}

#[test]
fn id_parse_failure_names_the_type_and_the_input() {
    let err = TurnId::from_str("not-a-uuid").unwrap_err();
    assert!(matches!(
        &err,
        ProtocolError::InvalidId { id_type: "TurnId", value, .. } if value == "not-a-uuid"
    ));
    assert_eq!(err.to_string(), "\"not-a-uuid\" is not a valid TurnId");
    assert!(err.source().is_some());
}

#[test]
fn id_serializes_as_a_json_string() {
    assert_eq!(serde_json::to_string(&conversation()).unwrap(), format!("\"{TEXT}\""));
}

#[test]
fn id_reads_uppercase_and_writes_lowercase() {
    let upper = format!("\"{}\"", TEXT.to_uppercase());
    let id: ConversationId = serde_json::from_str(&upper).unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{TEXT}\""));
}

#[test]
fn id_rejects_a_json_value_that_is_not_a_uuid() {
    assert!(serde_json::from_str::<ConversationId>("\"abc\"").is_err());
    assert!(serde_json::from_str::<ConversationId>("42").is_err());
}

#[test]
fn ids_sort_by_their_uuid_so_version_7_ids_sort_by_time() {
    let earlier = ConversationId::from_uuid(Uuid::from_u128(1));
    let later = ConversationId::from_uuid(Uuid::from_u128(2));
    assert!(earlier < later);
}

#[test]
fn seq_serializes_as_a_json_number() {
    assert_eq!(serde_json::to_string(&Seq::new(41)).unwrap(), "41");
    assert_eq!(serde_json::from_str::<Seq>("41").unwrap(), Seq::new(41));
}

#[test]
fn seq_starts_at_zero_and_orders_numerically() {
    assert_eq!(Seq::default(), Seq::ZERO);
    assert!(Seq::new(9) < Seq::new(10));
    assert_eq!(Seq::new(10).get(), 10);
    assert_eq!(Seq::new(10).to_string(), "10");
}

#[test]
fn seq_rejects_negative_numbers() {
    assert!(serde_json::from_str::<Seq>("-1").is_err());
}

#[test]
fn request_id_serializes_as_a_json_number() {
    assert_eq!(serde_json::to_string(&RequestId::new(7)).unwrap(), "7");
    assert_eq!(serde_json::from_str::<RequestId>("7").unwrap(), RequestId::new(7));
    assert_eq!(RequestId::new(7).get(), 7);
    assert_eq!(RequestId::new(7).to_string(), "7");
}
