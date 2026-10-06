use std::path::PathBuf;

use pretty_assertions::assert_eq;

use crate::SandboxError;
use crate::records::{RECORDS_HEADER, Records, encode_apply, encode_records, parse_records};
use crate::spec::RecordLimits;

fn stream(fields: &[&str]) -> Vec<u8> {
    let mut out = RECORDS_HEADER.to_vec();
    for field in fields {
        out.extend_from_slice(field.as_bytes());
        out.push(0);
    }
    out
}

#[test]
fn a_full_stream_reads_back() {
    let records = Records {
        cwd: Some("/home/u/p/app/src".into()),
        exports: vec![("RUST_LOG".to_owned(), "debug".to_owned())],
        unsets: vec!["OLD".to_owned()],
        functions: vec![("mk".to_owned(), "make -j8".to_owned())],
        removed_functions: vec!["gone".to_owned()],
        aliases: vec![("ll".to_owned(), "ls -l".to_owned())],
        removed_aliases: vec!["la".to_owned()],
        setup_error: None,
        status: Some(3),
    };
    let parsed = parse_records(&encode_records(&records), &RecordLimits::default()).unwrap();
    assert_eq!(parsed, records);
}

#[test]
fn records_reject_oversize() {
    let limits = RecordLimits { max_bytes: 64, max_records: 4096, max_value: 4096 };
    let big = stream(&["export", "A", &"x".repeat(100), "end", "0"]);
    assert!(matches!(parse_records(&big, &limits), Err(SandboxError::TooLarge { .. })));
    let limits = RecordLimits { max_bytes: 1 << 20, max_records: 2, max_value: 4096 };
    let many = stream(&["unset", "A", "unset", "B", "unset", "C", "end", "0"]);
    assert!(matches!(parse_records(&many, &limits), Err(SandboxError::TooManyRecords { max: 2 })));
}

#[test]
fn records_reject_missing_end() {
    let cut = stream(&["cwd", "/tmp", "export", "A", "1"]);
    assert!(matches!(
        parse_records(&cut, &RecordLimits::default()),
        Err(SandboxError::RecordsMissingEnd)
    ));
    let mut torn = stream(&["export", "A"]);
    torn.extend_from_slice(b"no-nul");
    assert!(parse_records(&torn, &RecordLimits::default()).is_err());
    assert!(matches!(
        parse_records(b"nope\0", &RecordLimits::default()),
        Err(SandboxError::RecordsHeader)
    ));
    let unknown = stream(&["eval", "rm -rf ~", "end", "0"]);
    assert!(matches!(
        parse_records(&unknown, &RecordLimits::default()),
        Err(SandboxError::RecordMalformed { .. })
    ));
}

#[test]
fn records_reject_control_chars_in_cwd() {
    for cwd in ["/tmp/a\nb", "/tmp/\u{1b}[2J", "relative/dir"] {
        let bytes = stream(&["cwd", cwd, "end", "0"]);
        assert!(
            matches!(parse_records(&bytes, &RecordLimits::default()), Err(SandboxError::RecordCwd)),
            "{cwd:?}"
        );
    }
}

#[test]
fn records_report_removed_functions() {
    let bytes = stream(&["unfunc", "precmd_theme", "unalias", "gst", "end", "0"]);
    let records = parse_records(&bytes, &RecordLimits::default()).unwrap();
    assert_eq!(records.removed_functions, ["precmd_theme"]);
    assert_eq!(records.removed_aliases, ["gst"]);
    assert_eq!(records.status, Some(0));
}

#[test]
fn a_setup_error_first_ends_the_stream_without_end() {
    let bytes = stream(&["setup-error", "landlock: ABI 6"]);
    let records = parse_records(&bytes, &RecordLimits::default()).unwrap();
    assert_eq!(records.setup_error.as_deref(), Some("landlock: ABI 6"));
    // Later, it is just a malformed stream.
    let later = stream(&["cwd", "/tmp", "setup-error", "x", "end", "0"]);
    assert!(parse_records(&later, &RecordLimits::default()).is_err());
}

#[test]
fn the_apply_file_holds_cd_exports_and_unsets() {
    let bytes = encode_apply(
        Some(&PathBuf::from("/home/u/p/app")),
        &[("RUST_LOG".to_owned(), "debug".to_owned())],
        &["OLD".to_owned()],
    );
    assert_eq!(bytes, b"cd\0/home/u/p/app\0export\0RUST_LOG\0debug\0unset\0OLD\0");
}

#[test]
fn debug_shows_names_not_values() {
    let records = Records {
        exports: vec![("API_TOKEN".to_owned(), "sk-secret-value".to_owned())],
        functions: vec![("deploy".to_owned(), "curl -H secret-body".to_owned())],
        aliases: vec![("ll".to_owned(), "ls -l secret-alias".to_owned())],
        ..Records::default()
    };
    let shown = format!("{records:?}");
    assert!(shown.contains("API_TOKEN") && shown.contains("deploy") && shown.contains("ll"));
    assert!(!shown.contains("secret"), "{shown}");
}
