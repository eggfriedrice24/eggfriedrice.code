use http::{HeaderMap, HeaderValue};
use pretty_assertions::assert_eq;
use rstest::rstest;

use super::TailscaleVersion;

#[rstest]
#[case::release("1.104.1", Some((1, 104, 1)))]
#[case::long("1.104.1-t0123abcd-g4567ef89", Some((1, 104, 1)))]
#[case::dev("1.105.0-dev20261025-t0123abcd", Some((1, 105, 0)))]
#[case::spaces(" 1.100.0 ", Some((1, 100, 0)))]
#[case::two_parts("1.104", None)]
#[case::four_parts("1.104.1.2", None)]
#[case::words("unknown", None)]
#[case::empty("", None)]
#[case::negative("1.-1.0", None)]
fn parse_reads_major_minor_and_patch(
    #[case] text: &str,
    #[case] expected: Option<(u32, u32, u32)>,
) {
    assert_eq!(TailscaleVersion::parse(text).map(|version| version.triple()), expected);
}

#[test]
fn the_long_text_is_kept_and_display_is_short() {
    let version = TailscaleVersion::parse("1.104.1-t0123abcd-g4567ef89").unwrap();
    assert_eq!(version.long(), "1.104.1-t0123abcd-g4567ef89");
    assert_eq!(version.to_string(), "1.104.1");
}

#[test]
fn at_least_compares_in_order() {
    let minimum = TailscaleVersion::MINIMUM_REMOTE;
    assert_eq!(minimum, (1, 100, 0));
    let at = |text: &str| TailscaleVersion::parse(text).unwrap().at_least(minimum);
    assert!(at("1.100.0"));
    assert!(at("1.104.1"));
    assert!(at("2.0.0"));
    assert!(!at("1.98.4"));
    assert!(!at("0.200.0"));
}

#[test]
fn from_headers_reads_the_tailscale_version_header() {
    let mut headers = HeaderMap::new();
    assert_eq!(TailscaleVersion::from_headers(&headers), None);
    headers.insert("tailscale-version", HeaderValue::from_static("1.104.1-tabc"));
    assert_eq!(TailscaleVersion::from_headers(&headers).unwrap().triple(), (1, 104, 1));
    headers.insert("tailscale-version", HeaderValue::from_static("garbage"));
    assert_eq!(TailscaleVersion::from_headers(&headers), None);
}
