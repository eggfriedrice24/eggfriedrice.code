use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::json;

use super::{
    AttachmentId, AttachmentKind, AttachmentOrigin, KEPT_EXTENSION_BYTES, NAME_MAX_BYTES,
    WithheldReason, fit_name,
};

#[rstest]
#[case::a_name_that_fits("shot.png", "shot.png")]
#[case::exactly_the_limit(&"x".repeat(NAME_MAX_BYTES), &"x".repeat(NAME_MAX_BYTES))]
#[case::a_long_stem(
    &format!("{}.png", "x".repeat(300)),
    &format!("{}.png", "x".repeat(NAME_MAX_BYTES - 4)),
)]
#[case::four_byte_characters(
    &format!("{}.jpeg", "\u{1f600}".repeat(100)),
    &format!("{}.jpeg", "\u{1f600}".repeat(58)),
)]
#[case::no_extension(&"y".repeat(300), &"y".repeat(NAME_MAX_BYTES))]
#[case::a_dot_at_the_start_is_no_extension(
    &format!(".{}", "z".repeat(300)),
    &format!(".{}", "z".repeat(NAME_MAX_BYTES - 1)),
)]
#[case::a_long_extension_is_cut_as_the_stem(
    &format!("a.{}", "e".repeat(300)),
    &format!("a.{}", "e".repeat(NAME_MAX_BYTES - 2)),
)]
fn a_long_name_keeps_its_extension(#[case] name: &str, #[case] want: &str) {
    let fit = fit_name(name);
    assert_eq!(fit, want);
    assert!(fit.len() <= NAME_MAX_BYTES);
}

#[test]
fn the_longest_extension_that_a_cut_keeps_stays_whole() {
    let extension = format!(".{}", "e".repeat(KEPT_EXTENSION_BYTES - 1));
    let fit = fit_name(&format!("{}{extension}", "s".repeat(300)));
    assert!(fit.ends_with(&format!("s{extension}")), "{fit}");
    assert_eq!(fit.len(), NAME_MAX_BYTES);
}

#[test]
fn an_attachment_id_is_a_and_a_number() {
    assert_eq!(AttachmentId::nth(3).as_str(), "a3");
    assert_eq!(serde_json::to_value(AttachmentId::nth(12)).unwrap(), json!("a12"));
    assert_eq!(serde_json::from_value::<AttachmentId>(json!("a7")).unwrap().number(), Some(7));
    for bad in ["a0", "a", "j3", "a01", "a1x", "A1"] {
        assert!(bad.parse::<AttachmentId>().is_err(), "{bad:?} does not parse");
    }
    assert_eq!(
        "j3".parse::<AttachmentId>().unwrap_err().to_string(),
        "\"j3\" is not an attachment id"
    );
}

#[test]
fn a_kind_and_an_origin_that_this_build_does_not_know_read_as_unknown() {
    let kind: AttachmentKind = serde_json::from_value(json!({"kind": "video", "s": 3})).unwrap();
    assert_eq!(kind, AttachmentKind::Unknown);
    let origin: AttachmentOrigin = serde_json::from_value(json!({"kind": "drop"})).unwrap();
    assert_eq!(origin, AttachmentOrigin::Unknown);
    let reason: WithheldReason = serde_json::from_value(json!("lost")).unwrap();
    assert_eq!(reason, WithheldReason::Unknown);
}

#[test]
fn a_pdf_with_no_count_leaves_the_member_out() {
    let pdf = AttachmentKind::Pdf { pages: None };
    assert_eq!(serde_json::to_value(pdf).unwrap(), json!({"kind": "pdf"}));
    assert_eq!(serde_json::from_value::<AttachmentKind>(json!({"kind": "pdf"})).unwrap(), pdf);
}
