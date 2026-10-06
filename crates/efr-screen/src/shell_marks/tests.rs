use std::path::PathBuf;

use efr_protocol::Seq;
use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::{
    PromptKind, SemanticPromptEvent, ShellMark, ShellMarkKind, ShellMarkScanner, percent_decode,
};

fn mark(start: u64, end: u64, kind: ShellMarkKind) -> ShellMark {
    ShellMark { start: Seq::new(start), end: Seq::new(end), kind }
}

fn prompt(event: SemanticPromptEvent) -> ShellMarkKind {
    ShellMarkKind::SemanticPrompt(event)
}

fn command_end(exit_code: Option<i32>) -> ShellMarkKind {
    prompt(SemanticPromptEvent::CommandEnd { exit_code, error: None, aid: None })
}

fn output_start() -> ShellMarkKind {
    prompt(SemanticPromptEvent::OutputStart { command: None, aid: None })
}

/// Scans `chunks` in order as one stream that starts at offset `base`.
fn scan_chunks(base: u64, chunks: &[&[u8]]) -> Vec<ShellMark> {
    let mut scanner = ShellMarkScanner::new();
    let mut offset = base;
    let mut marks = Vec::new();
    for chunk in chunks {
        marks.extend(scanner.scan(chunk, Seq::new(offset)));
        offset += chunk.len() as u64;
    }
    marks
}

fn scan(bytes: &[u8]) -> Vec<ShellMark> {
    scan_chunks(0, &[bytes])
}

#[test]
fn a_command_cycle_gives_the_output_range() {
    let stream =
        b"\x1b]133;A;cl=line\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07a b\r\n\x1b]133;D;0\x07";
    let marks = scan(stream);
    assert_eq!(
        marks,
        vec![
            mark(
                0,
                16,
                prompt(SemanticPromptEvent::PromptStart {
                    kind: PromptKind::Initial,
                    aid: None,
                    click: Some(super::ClickMode::Line),
                    fresh_line: true,
                })
            ),
            mark(18, 26, prompt(SemanticPromptEvent::InputStart)),
            mark(30, 38, output_start()),
            mark(43, 53, command_end(Some(0))),
        ]
    );
    let output = &stream[marks[2].end.get() as usize..marks[3].start.get() as usize];
    assert_eq!(output, b"a b\r\n");
}

#[test]
fn a_mark_split_across_chunks_keeps_the_start_of_the_first_chunk() {
    let marks = scan_chunks(100, &[b"out\x1b]133;", b"D;", b"1\x07"]);
    assert_eq!(marks, vec![mark(103, 113, command_end(Some(1)))]);
}

#[test]
fn a_split_between_esc_and_bracket() {
    let marks = scan_chunks(0, &[b"x\x1b", b"]133;C\x07"]);
    assert_eq!(marks, vec![mark(1, 9, output_start())]);
}

#[test]
fn a_gap_in_the_recording_drops_a_partial_mark() {
    let mut scanner = ShellMarkScanner::new();
    assert_eq!(scanner.scan(b"\x1b]133;", Seq::new(0)), vec![]);
    // The chunk should have started at 6; the recording lost bytes in between.
    assert_eq!(
        scanner.scan(b"D;0\x07\x1b]133;C\x07", Seq::new(50)),
        vec![mark(54, 62, output_start())]
    );
}

#[test]
fn a_contiguous_chunk_does_not_reset() {
    let mut scanner = ShellMarkScanner::new();
    assert_eq!(scanner.scan(b"\x1b]133;", Seq::new(10)), vec![]);
    assert_eq!(scanner.scan(b"D\x07", Seq::new(16)), vec![mark(10, 18, command_end(None))]);
}

#[test]
fn the_first_chunk_may_start_anywhere() {
    let mut scanner = ShellMarkScanner::new();
    assert_eq!(
        scanner.scan(b"\x1b]133;B\x07", Seq::new(7)),
        vec![mark(7, 15, prompt(SemanticPromptEvent::InputStart))]
    );
}

#[test]
fn both_cwd_forms() {
    let file = b"\x1b]7;file://arch/home/egg%20x\x07";
    let kitty = b"\x1b]7;kitty-shell-cwd://arch/tmp\x1b\\";
    let (first, second) = (file.len() as u64, (file.len() + kitty.len()) as u64);
    let host = Some("arch".to_owned());
    assert_eq!(
        scan(&[file.as_slice(), kitty].concat()),
        vec![
            mark(
                0,
                first,
                ShellMarkKind::CwdChanged {
                    host: host.clone(),
                    path: PathBuf::from("/home/egg x")
                }
            ),
            mark(first, second, ShellMarkKind::CwdChanged { host, path: PathBuf::from("/tmp") }),
        ]
    );
}

#[test]
fn other_osc_numbers_are_not_marks() {
    assert_eq!(
        scan(b"\x1b]0;title\x07\x1b]2;t\x07\x1b]1337;SetMark\x07\x1b]52;c;aGk=\x07"),
        vec![]
    );
    assert_eq!(scan(b"\x1b]1330;A\x07\x1b]33;A\x07\x1b]0133;A\x07"), vec![]);
}

#[test]
fn malformed_marks_are_skipped() {
    assert_eq!(scan(b"\x1b]133\x07\x1b]133;\x07\x1b]133;Z\x07\x1b]7;\x07\x1b]7\x07"), vec![]);
}

#[test]
fn an_unclosed_command_closed_by_a_bare_d() {
    let marks = scan(b"\x1b]133;C\x07^C\r\n\x1b]133;D\x07");
    assert_eq!(marks, vec![mark(0, 8, output_start()), mark(12, 20, command_end(None))]);
}

#[test]
fn percent_decoding() {
    assert_eq!(percent_decode(b"a%20b%2fc%2F"), Some(b"a b/c/".to_vec()));
    assert_eq!(percent_decode(b"plain"), Some(b"plain".to_vec()));
    assert_eq!(percent_decode(b""), Some(Vec::new()));
    assert_eq!(percent_decode(b"%"), None);
    assert_eq!(percent_decode(b"%4"), None);
    assert_eq!(percent_decode(b"%4g"), None);
    assert_eq!(percent_decode(b"%+1"), None);
}

/// Well-formed mark sequences with the mark each one must produce, relative to its
/// own first byte.
#[test]
fn a_sandbox_end_mark_carries_its_nonce_and_a_bad_one_is_no_mark() {
    let stream =
        b"\x1b]133;efr-sbx;nothex\x07\x1b]133;efr-sbx;ffeeddccbbaa99887766554433221100\x1b\\";
    let nonce = [
        0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11,
        0x00,
    ];
    assert_eq!(scan(stream), vec![mark(21, 69, ShellMarkKind::SandboxEnd { nonce })]);
}

fn known_marks() -> Vec<(Vec<u8>, ShellMarkKind)> {
    let st = |body: &str| [b"\x1b]".as_slice(), body.as_bytes(), b"\x1b\\"].concat();
    let bel = |body: &str| [b"\x1b]".as_slice(), body.as_bytes(), b"\x07"].concat();
    vec![
        (
            bel("133;A;cl=line;aid=9"),
            prompt(SemanticPromptEvent::PromptStart {
                kind: PromptKind::Initial,
                aid: Some("9".to_owned()),
                click: Some(super::ClickMode::Line),
                fresh_line: true,
            }),
        ),
        (
            st("133;P;k=s"),
            prompt(SemanticPromptEvent::PromptStart {
                kind: PromptKind::Secondary,
                aid: None,
                click: None,
                fresh_line: false,
            }),
        ),
        (bel("133;B"), prompt(SemanticPromptEvent::InputStart)),
        (st("133;C"), output_start()),
        (bel("133;D;127"), command_end(Some(127))),
        (st("133;D"), command_end(None)),
        (
            bel("133;efr-sbx;0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f"),
            ShellMarkKind::SandboxEnd { nonce: [0x0f; 16] },
        ),
        (
            bel("7;kitty-shell-cwd://h/tmp/x"),
            ShellMarkKind::CwdChanged { host: Some("h".to_owned()), path: PathBuf::from("/tmp/x") },
        ),
        (
            st("7;file:///var/a%20b"),
            ShellMarkKind::CwdChanged { host: None, path: PathBuf::from("/var/a b") },
        ),
    ]
}

/// Output that holds no escape sequence and no control that aborts one.
fn plain_text() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(
        prop_oneof![0x20_u8..0x7f, Just(b'\r'), Just(b'\n'), Just(0x07_u8), 0x80_u8..=0xff],
        0..24,
    )
}

/// One piece of a stream: plain output, a known mark, or arbitrary bytes, which may
/// open, break or close sequences anywhere.
fn piece() -> impl Strategy<Value = Vec<u8>> {
    let marks: Vec<Vec<u8>> = known_marks().into_iter().map(|(bytes, _)| bytes).collect();
    prop_oneof![
        plain_text(),
        proptest::sample::select(marks),
        proptest::collection::vec(any::<u8>(), 0..16),
        proptest::sample::select(vec![
            b"\x1b".to_vec(),
            b"\x1b]".to_vec(),
            b"\x1b]133;".to_vec(),
            b"\x1bP".to_vec(),
            b"\x1b\\".to_vec(),
            b"\x18".to_vec(),
        ]),
    ]
}

/// Cuts `bytes` at the given fractions of its length.
fn split<'a>(bytes: &'a [u8], cuts: &[prop::sample::Index]) -> Vec<&'a [u8]> {
    let mut points: Vec<usize> = cuts.iter().map(|cut| cut.index(bytes.len() + 1)).collect();
    points.sort_unstable();
    points.dedup();
    let mut chunks = Vec::new();
    let mut from = 0;
    for point in points {
        chunks.push(&bytes[from..point]);
        from = point;
    }
    chunks.push(&bytes[from..]);
    chunks
}

proptest! {
    #[test]
    fn any_split_of_any_stream_yields_the_same_marks(
        pieces in proptest::collection::vec(piece(), 0..24),
        cuts in proptest::collection::vec(any::<prop::sample::Index>(), 0..12),
        base in 0_u64..1_000_000,
    ) {
        let stream = pieces.concat();
        let whole = scan_chunks(base, &[&stream]);
        prop_assert_eq!(scan_chunks(base, &split(&stream, &cuts)), whole.clone());
        let one_byte: Vec<&[u8]> = stream.chunks(1).collect();
        prop_assert_eq!(scan_chunks(base, &one_byte), whole);
    }

    #[test]
    fn a_well_formed_stream_yields_exactly_its_marks(
        layout in proptest::collection::vec((plain_text(), 0..8_usize), 0..12),
        tail in plain_text(),
        cuts in proptest::collection::vec(any::<prop::sample::Index>(), 0..12),
        base in 0_u64..1_000_000,
    ) {
        let known = known_marks();
        let mut stream = Vec::new();
        let mut expected = Vec::new();
        for (text, choice) in layout {
            stream.extend_from_slice(&text);
            let (bytes, kind) = &known[choice % known.len()];
            let start = base + stream.len() as u64;
            stream.extend_from_slice(bytes);
            expected.push(mark(start, base + stream.len() as u64, kind.clone()));
        }
        stream.extend_from_slice(&tail);
        prop_assert_eq!(scan_chunks(base, &split(&stream, &cuts)), expected);
    }
}
