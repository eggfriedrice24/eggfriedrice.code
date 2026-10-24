use efr_protocol::{
    ConversationId, ConversationSearchResult, ConversationStatus, ConversationSummary,
    ConversationsSearchResult, PageCursor, ProjectId, RecentConversation, RouteScope, SearchHit,
    SearchIndexState, SearchKind, SearchLine, SearchPartial, SearchRow, Seq, TextRange,
};
use efr_render::{ColourMode, RenderOptions};
use jiff::Timestamp;
use pretty_assertions::assert_eq;
use proptest::prelude::{prop, proptest};

use super::{
    Marked, ROW_BYTES, clamp_ranges, list, picker_header, picker_rows, preview, preview_failure,
};
use crate::testing::{bare, readable};

fn now() -> Timestamp {
    "2026-10-20T12:00:00Z".parse().unwrap()
}

fn ago(minutes: i64) -> Timestamp {
    now() - jiff::SignedDuration::from_mins(minutes)
}

fn id(n: u64) -> ConversationId {
    format!("019a9b1c-3d00-7a10-8b20-{n:012x}").parse().unwrap()
}

fn app() -> RouteScope {
    RouteScope::Project("0192f0c1-7a00-7000-8000-0000000a4401".parse::<ProjectId>().unwrap())
}

fn ranges(pairs: &[(u32, u32)]) -> Vec<TextRange> {
    pairs.iter().map(|&(start, end)| TextRange { start, end }).collect()
}

fn line(text: &str, pairs: &[(u32, u32)]) -> SearchLine {
    SearchLine { text: text.to_owned(), ranges: ranges(pairs) }
}

fn hit(kind: SearchKind, minutes: i64, lines: Vec<SearchLine>) -> SearchHit {
    SearchHit {
        kind,
        seq: Seq::new(40),
        turn_id: None,
        at: ago(minutes),
        history_cursor: PageCursor::new("c40"),
        lines,
    }
}

/// A row of conversation `n`, `minutes` old, with `turns` turns.
fn row(n: u64, title: Option<&str>, minutes: i64, turns: u32) -> SearchRow {
    SearchRow {
        conversation: RecentConversation {
            summary: ConversationSummary {
                id: id(n),
                title: title.map(str::to_owned),
                status: ConversationStatus::Idle,
                created_at: ago(minutes + 60),
                updated_at: ago(minutes),
                last_seq: Seq::new(n),
                cwd: None,
                scope: None,
                tty: None,
            },
            route: RouteScope::Outside,
            project: None,
            turns,
            open_in: None,
            here: false,
        },
        short_id: format!("019a9b1{n}"),
        title_ranges: Vec::new(),
        hit: None,
    }
}

/// Three rows for `rsync lap`: a match in the title and a prompt, a match in a command
/// only, and one in the title only.
fn found() -> ConversationsSearchResult {
    let mut first = row(1, Some("rsync the photos to the laptop"), 25, 6);
    first.conversation.route = app();
    first.conversation.project = Some("app".to_owned());
    first.conversation.here = true;
    first.title_ranges = ranges(&[(0, 5), (24, 27)]);
    first.hit = Some(hit(
        SearchKind::Prompt,
        25,
        vec![line("please rsync ~/photos to the laptop and check its disk", &[(7, 12), (29, 32)])],
    ));
    let mut second = row(2, Some("why is the build slow"), 125, 3);
    second.conversation.open_in = Some("/dev/pts/4".to_owned());
    second.hit = Some(hit(
        SearchKind::Command,
        125,
        vec![line("rsync -a target/ lap:/srv/cache", &[(0, 5), (17, 20)])],
    ));
    let mut third = row(3, Some("set up the laptop"), 4_400, 1);
    third.title_ranges = ranges(&[(11, 14)]);
    ConversationsSearchResult { rows: vec![first, second, third], ..Default::default() }
}

fn terminal() -> RenderOptions {
    RenderOptions::new(80)
}

fn pipe() -> RenderOptions {
    RenderOptions::new(80).with_terminal(false)
}

#[test]
fn ranges_are_clamped_sorted_and_merged() {
    let text = "héllo wörld";
    let none: Vec<std::ops::Range<usize>> = Vec::new();
    // Code points: h0 é1 l2 l3 o4 _5 w6 ö7 r8 l9 d10; é and ö take two bytes.
    assert_eq!(clamp_ranges(text, &ranges(&[(6, 11)])), vec![7..13]);
    assert_eq!(clamp_ranges(text, &ranges(&[(6, 99)])), vec![7..13], "cut to the text");
    assert_eq!(clamp_ranges(text, &ranges(&[(3, 3), (5, 2)])), none, "empty and reversed");
    assert_eq!(clamp_ranges(text, &ranges(&[(4, 6), (0, 2), (1, 3)])), vec![0..4, 5..7]);
    assert_eq!(clamp_ranges(text, &ranges(&[(0, 2), (2, 3)])), vec![0..4], "touching ones merge");
    assert_eq!(clamp_ranges(text, &ranges(&[(u32::MAX - 1, u32::MAX)])), none);
    assert_eq!(clamp_ranges("", &ranges(&[(0, 4)])), none);
}

#[test]
fn picker_rows_with_matches() {
    let text = picker_rows(&found(), now(), &terminal());
    insta::assert_snapshot!(readable(&text));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "the header and three rows");
    assert!(lines[0].starts_with('\t'), "the header has an empty id");
    assert!(lines[1].starts_with(&format!("{}\t", id(1))));
}

#[test]
fn picker_rows_without_matches() {
    let mut recent = found();
    for row in &mut recent.rows {
        row.title_ranges.clear();
        row.hit = None;
    }
    let text = picker_rows(&recent, now(), &terminal());
    insta::assert_snapshot!(readable(&text));
    for line in text.lines() {
        assert_eq!(line, line.trim_end(), "no blanks at the end of a row");
    }
}

#[test]
fn picker_header_says_how_far_the_index_is_and_why_the_search_stopped() {
    let result = ConversationsSearchResult {
        partial: Some(SearchPartial::TimeCap),
        index: Some(SearchIndexState { percent: 40 }),
        ..Default::default()
    };
    insta::assert_snapshot!(
        picker_rows(&result, now(), &pipe()),
        @"\tenter resume · ctrl-o print · ctrl-d delete · esc close   indexing: 40% · partial: time cap"
    );
    let result =
        ConversationsSearchResult { partial: Some(SearchPartial::RowCap), ..Default::default() };
    insta::assert_snapshot!(
        picker_rows(&result, now(), &pipe()),
        @"\tenter resume · ctrl-o print · ctrl-d delete · esc close   partial: row cap"
    );
    assert_eq!(
        picker_header(None, &pipe()),
        "\tenter resume · ctrl-o print · ctrl-d delete · esc close\n"
    );
    insta::assert_snapshot!(
        readable(&picker_header(Some("efrd does not answer"), &terminal())).replace('\t', "\\t"),
        @r"\t\e[1menter\e[0m\e[2m resume\e[0m\e[2m · \e[0m\e[1mctrl-o\e[0m\e[2m print\e[0m\e[2m · \e[0m\e[1mctrl-d\e[0m\e[2m delete\e[0m\e[2m · \e[0m\e[1mesc\e[0m\e[2m close\e[0m   \e[2mefrd does not answer\e[0m"
    );
}

#[test]
fn a_title_with_control_characters_stays_on_its_row() {
    let mut result = found();
    let evil = "fix\tthe\ntests\x1b]52;c;ZXZpbA==\x07 now\u{202e}";
    result.rows[0].conversation.summary.title = Some(evil.to_owned());
    result.rows[0].title_ranges = ranges(&[(0, 3), (40, 99)]);
    result.rows[0].hit = Some(hit(SearchKind::Answer, 5, vec![line(evil, &[(4, 7)])]));
    for text in [
        picker_rows(&result, now(), &terminal()),
        list(&result, now(), false, &terminal()),
        list(&result, now(), false, &pipe()),
    ] {
        let lines: Vec<&str> = text.lines().collect();
        assert!(!text.contains("\x1b]"), "no OSC: {}", readable(&text));
        assert!(!text.contains('\x07') && !text.contains('\u{202e}'), "{}", readable(&text));
        assert!(bare(&text).contains("fix the tests␛]52;c;ZXZpbA==␇ now"), "{}", readable(&text));
        for line in lines {
            assert!(line.matches('\t').count() <= 1, "only the tab after the id: {line:?}");
        }
    }
}

#[test]
fn preview_at_80_columns() {
    let result = ConversationSearchResult {
        title: Some(line("rsync the photos to the laptop", &[(0, 5)])),
        hits: vec![
            hit(
                SearchKind::Prompt,
                25,
                vec![line("please rsync ~/photos to the laptop and check its disk", &[(7, 12)])],
            ),
            hit(
                SearchKind::Answer,
                24,
                vec![
                    line(
                        "I ran rsync with -a, so the times and the modes stay as they were on the source.",
                        &[(6, 11)],
                    ),
                    line("Then I checked the disk of the laptop: 41% is free.", &[]),
                ],
            ),
            hit(
                SearchKind::Command,
                23,
                vec![line("rsync -a ~/photos laptop:/srv/photos", &[(0, 5)])],
            ),
        ],
        partial: None,
    };
    let text = preview(&result, "rsync", now(), 80, &terminal());
    insta::assert_snapshot!(readable(&text));
    for line in bare(&text).lines() {
        assert!(line.chars().count() <= 80, "{line}");
    }
}

#[test]
fn preview_at_40_columns_keeps_a_late_match_in_view() {
    let long = "the first part of this answer says nothing of interest, and then rsync comes";
    let result = ConversationSearchResult {
        title: None,
        hits: vec![hit(SearchKind::Answer, 90, vec![line(long, &[(66, 71)])])],
        partial: Some(SearchPartial::TimeCap),
    };
    let text = preview(&result, "rsync", now(), 40, &pipe());
    insta::assert_snapshot!(text);
    for line in text.lines() {
        assert!(line.chars().count() <= 40, "{line}");
    }
    assert!(text.contains("rsync"), "the match shows");
}

#[test]
fn preview_without_a_match_says_so() {
    let result = ConversationSearchResult {
        title: Some(line("set up the laptop", &[(11, 14)])),
        ..Default::default()
    };
    insta::assert_snapshot!(
        preview(&result, "lap", now(), 80, &pipe()),
        @r"
    set up the laptop

    no prompt, answer or command matches
    "
    );
    assert_eq!(preview(&result, "", now(), 80, &pipe()), "set up the laptop\n");
    assert_eq!(preview_failure("efrd does not answer", 80, &pipe()), "efrd does not answer\n");
}

#[test]
fn plain_list_at_80_columns() {
    let text = list(&found(), now(), false, &terminal());
    insta::assert_snapshot!(readable(&text));
    for line in bare(&text).lines() {
        assert!(line.chars().count() <= 80, "{line}");
    }
    assert!(text.ends_with("efr resume <id> continues one here\x1b[0m\n"), "{}", readable(&text));
}

#[test]
fn plain_list_in_a_pipe() {
    let mut result = found();
    result.index = Some(SearchIndexState { percent: 7 });
    let long = format!("{} and at last the word rsync", "words before the match ".repeat(8));
    result.rows[1].hit = Some(hit(SearchKind::Answer, 125, vec![line(&long, &[(208, 213)])]));
    let text = list(&result, now(), true, &pipe());
    insta::assert_snapshot!(text);
    assert!(!text.contains('\x1b'), "no colours in a pipe");
    assert!(text.contains(&long), "no cut in a pipe");
    assert!(text.ends_with(",resume <id> continues one here\n"));
}

#[test]
fn plain_list_without_rows_says_so() {
    let empty = ConversationsSearchResult::default();
    assert_eq!(list(&empty, now(), false, &pipe()), "no conversation matches\n");
    let indexing =
        ConversationsSearchResult { index: Some(SearchIndexState { percent: 3 }), ..empty };
    assert_eq!(list(&indexing, now(), false, &pipe()), "no conversation matches\nindexing: 3%\n");
}

#[test]
fn fifty_long_rows_fit_in_32_kib() {
    // The worst rows: text of four-byte characters that take no column, a match on
    // every second character, and 24-bit colours.
    let heavy = "\u{1d165}x".repeat(300);
    let every_other: Vec<(u32, u32)> = (0..300).map(|n| (n * 2 + 1, n * 2 + 2)).collect();
    let rows = (1..=50)
        .map(|n| {
            let mut row = row(n, Some(&heavy), 30, 999_999);
            row.title_ranges = ranges(&every_other);
            row.conversation.route = app();
            row.conversation.project = Some(heavy.clone());
            row.conversation.open_in = Some(heavy.clone());
            row.short_id = heavy.clone();
            row.hit = Some(hit(SearchKind::Answer, 30, vec![line(&heavy, &every_other)]));
            row
        })
        .collect();
    let result = ConversationsSearchResult {
        rows,
        partial: Some(SearchPartial::TimeCap),
        index: Some(SearchIndexState { percent: 99 }),
        ..Default::default()
    };
    let options = RenderOptions::new(200).with_colour(ColourMode::TrueColor);
    let text = picker_rows(&result, now(), &options);
    assert_eq!(text.lines().count(), 51);
    assert!(text.len() <= 32 * 1024, "{} bytes", text.len());
    for line in text.lines().skip(1) {
        assert!(line.len() < ROW_BYTES, "{} bytes", line.len());
    }
    // And the rows of a real search: titles of 80 characters, long snippets.
    let words = "rsync the photos of the laptop to the backup disk and check the copy ";
    let rows = (1..=50)
        .map(|n| {
            let mut row = row(n, Some(&words[..80.min(words.len())]), 30, 12);
            row.title_ranges = ranges(&[(0, 5), (24, 30)]);
            row.hit = Some(hit(
                SearchKind::Prompt,
                30,
                vec![line(&words.repeat(6), &[(0, 5), (24, 30), (69, 74)])],
            ));
            row
        })
        .collect();
    let result = ConversationsSearchResult { rows, ..Default::default() };
    let text = picker_rows(&result, now(), &options);
    assert!(text.len() <= 32 * 1024, "{} bytes", text.len());
}

proptest! {
    #[test]
    fn clamped_ranges_never_panic_and_stay_on_characters(
        text in ".{0,40}",
        pairs in prop::collection::vec((0_u32..60, 0_u32..60), 0..8),
    ) {
        let spans = clamp_ranges(&text, &ranges(&pairs));
        let mut last = 0;
        for span in &spans {
            assert!(span.start < span.end && span.end <= text.len());
            assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end));
            assert!(span.start >= last, "sorted and apart");
            last = span.end;
        }
    }

    #[test]
    fn marked_text_never_panics(
        text in ".{0,60}",
        pairs in prop::collection::vec((0_u32..80, 0_u32..80), 0..8),
        columns in 0_usize..50,
        bytes in 0_usize..120,
        budget in 0_usize..200,
    ) {
        let marked = Marked::new(&text, &ranges(&pairs));
        let options = RenderOptions::new(80);
        for lead in [false, true] {
            let window = marked.window(columns, bytes, lead, options.width_method());
            for hit in &window.hits {
                assert!(window.text.get(hit.clone()).is_some());
            }
            let painted = window.paint_within(super::Tone::Dim, &options, budget);
            assert!(painted.len() <= budget, "{} bytes over {budget}", painted.len());
        }
    }
}
