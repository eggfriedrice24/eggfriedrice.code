use bytes::Bytes;
use efr_screen::{
    Color, Cursor, RowCells, Screen, ScreenActor, ScreenEvent, ScreenText, Seq, Size, cells_text,
};
use pretty_assertions::assert_eq;

use super::{GhosttyScreen, SCROLLBACK_LINES, factory, restore_factory};
use crate::GhosttyError;
use crate::testing::{Call, Fed, Recorder, texts};

fn size(cols: u16, rows: u16) -> Size {
    Size { cols, rows }
}

fn screen(cols: u16, rows: u16) -> Fed {
    Fed::new(GhosttyScreen::new(size(cols, rows)).unwrap())
}

fn restore(snapshot: &[u8]) -> Fed {
    Fed::new(GhosttyScreen::restore(snapshot).unwrap())
}

/// libghostty-vt's working directory: the raw OSC 7 URL as the program sent it.
fn pwd(screen: &GhosttyScreen) -> Option<String> {
    let pwd = str::from_utf8(screen.terminal.pwd().ok()?).ok()?;
    (!pwd.is_empty()).then(|| pwd.to_owned())
}

fn cursor(screen: &mut GhosttyScreen) -> Cursor {
    screen.snapshot(0).cursor
}

fn title(screen: &mut GhosttyScreen) -> Option<String> {
    screen.snapshot(0).title
}

/// Visible row `index`, as the backend reads it.
fn row(screen: &mut GhosttyScreen, index: usize) -> RowCells {
    screen.snapshot(0).rows.swap_remove(index)
}

#[test]
fn a_new_screen_is_blank_at_its_size() {
    let mut screen = screen(10, 3);
    let snapshot = screen.snapshot(100);
    assert_eq!(snapshot.size, size(10, 3));
    assert_eq!(texts(&snapshot.rows), vec!["", "", ""]);
    assert_eq!(snapshot.scrollback, Vec::new());
    assert_eq!(cursor(&mut screen), Cursor { row: 0, col: 0, hidden: false });
    assert_eq!(title(&mut screen), None);
    assert_eq!(pwd(&screen), None);
}

#[test]
fn a_zero_size_becomes_one_cell() {
    let mut screen = screen(0, 0);
    assert_eq!(screen.snapshot(0).size, size(1, 1));
    screen.resize(0, 5, &mut Recorder::default());
    assert_eq!(screen.snapshot(0).size, size(1, 5));
}

#[test]
fn feed_renders_text_and_moves_the_cursor() {
    let mut screen = screen(10, 3);
    let sink = screen.feed(b"hello\r\nworld");
    assert_eq!(sink.calls, Vec::new());
    assert_eq!(texts(&screen.snapshot(0).rows), vec!["hello", "world", ""]);
    assert_eq!(cursor(&mut screen), Cursor { row: 1, col: 5, hidden: false });
}

#[test]
fn terminal_queries_are_answered_through_the_sink() {
    let mut screen = screen(10, 2);
    assert_eq!(screen.feed(b"\x1b[c").replies(), b"\x1b[?62;22c".to_vec());
    assert_eq!(screen.feed(b"ab\x1b[6n").replies(), b"\x1b[1;3R".to_vec());
    assert_eq!(screen.feed(b"\x1b[?7$p").replies(), b"\x1b[?7;1$y".to_vec());
}

#[test]
fn osc_10_and_11_are_answered_with_ghosttys_colours() {
    let mut screen = screen(10, 2);
    let sink = screen.feed(b"\x1b]10;?\x07\x1b]11;?\x1b\\");
    assert_eq!(
        sink.replies(),
        b"\x1b]10;rgb:ffff/ffff/ffff\x07\x1b]11;rgb:2828/2c2c/3434\x1b\\".to_vec()
    );
}

#[test]
fn bells_and_titles_reach_the_sink_in_order() {
    let mut screen = screen(10, 2);
    let sink = screen.feed(b"\x07\x1b]2;make\x07\x07");
    assert_eq!(sink.calls, vec![Call::Bell, Call::Title("make".to_owned()), Call::Bell]);
    assert_eq!(title(&mut screen).as_deref(), Some("make"));
}

#[test]
fn pwd_is_the_raw_osc_7_url() {
    let mut screen = screen(10, 2);
    screen.feed(b"\x1b]7;file://arch/home/egg/My%20Docs\x07");
    assert_eq!(pwd(&screen).as_deref(), Some("file://arch/home/egg/My%20Docs"));
    screen.feed(b"\x1b]7;kitty-shell-cwd://arch/tmp/x y\x07");
    assert_eq!(pwd(&screen).as_deref(), Some("kitty-shell-cwd://arch/tmp/x y"));
}

#[test]
fn a_disagreement_with_the_scanner_never_fails_a_feed() {
    let mut screen = screen(10, 2);
    // ghostty takes an http:// OSC 7 that the scanner refuses: a disagreement.
    let sink = screen.feed(b"\x1b]7;http://arch/tmp\x07ok\x1b[c");
    assert_eq!(sink.replies(), b"\x1b[?62;22c".to_vec());
    assert_eq!(texts(&[row(&mut screen, 0)]), vec!["ok"]);
    assert_eq!(pwd(&screen).as_deref(), Some("http://arch/tmp"));
}

/// The Kitty graphics support query: can the terminal take one RGB pixel as image 31?
const KITTY_QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";

/// A Kitty graphics command that stores a black RGB image of `width` x `height` pixels
/// as image `id`. Its APC holds about `4 * width * height` bytes.
fn kitty_image(id: u32, width: u32, height: u32) -> Vec<u8> {
    let mut apc = format!("\x1b_Ga=t,f=24,s={width},v={height},i={id};").into_bytes();
    // Three zero bytes are "AAAA" in base64.
    for _ in 0..width * height {
        apc.extend_from_slice(b"AAAA");
    }
    apc.extend_from_slice(b"\x1b\\");
    apc
}

#[test]
fn the_kitty_graphics_query_gets_no_answer_on_either_screen() {
    let mut screen = screen(10, 2);
    assert_eq!(screen.feed(KITTY_QUERY).replies(), Vec::<u8>::new());
    // The alternate screen starts later, with the limit of the primary screen.
    screen.feed(b"\x1b[?1049h");
    assert_eq!(screen.feed(KITTY_QUERY).replies(), Vec::<u8>::new());
    // With an image store, the same query gets an answer: the setting is what stops it.
    screen.terminal.set_kitty_image_storage_limit(10_000_000).unwrap();
    assert_eq!(screen.feed(KITTY_QUERY).replies(), b"\x1b_Gi=31;OK\x1b\\".to_vec());
}

#[test]
fn the_glyph_protocol_query_gets_no_answer() {
    let query = b"\x1b_25a1;s\x1b\\";
    let mut screen = screen(10, 2);
    assert_eq!(screen.feed(query).replies(), Vec::<u8>::new());
    screen.terminal.set_glyph_protocol_enabled(true).unwrap();
    assert!(!screen.feed(query).replies().is_empty());
}

#[test]
fn an_apc_longer_than_the_limit_is_dropped() {
    let mut screen = screen(10, 2);
    // An image store for this test only, so that a stored image is answered with OK.
    screen.terminal.set_kitty_image_storage_limit(10_000_000).unwrap();
    assert_eq!(screen.feed(&kitty_image(7, 1, 1)).replies(), b"\x1b_Gi=7;OK\x1b\\".to_vec());
    let long = kitty_image(8, 64, 64);
    assert!(long.len() > super::APC_LIMIT);
    assert_eq!(screen.feed(&long).replies(), Vec::<u8>::new());
    // The terminal reads on after the dropped APC.
    screen.feed(b"ok");
    assert_eq!(texts(&[row(&mut screen, 0)]), vec!["ok"]);
    // With the library default, the same image is stored: the limit is what drops it.
    screen.terminal.set_apc_max_bytes(None).unwrap();
    assert_eq!(screen.feed(&kitty_image(9, 64, 64)).replies(), b"\x1b_Gi=9;OK\x1b\\".to_vec());
}

#[test]
fn resize_changes_the_grid_and_keeps_the_text() {
    let mut screen = screen(5, 2);
    screen.feed(b"hey");
    let mut sink = Recorder::default();
    screen.resize(10, 3, &mut sink);
    assert_eq!(sink.calls, Vec::new());
    let snapshot = screen.snapshot(0);
    assert_eq!(snapshot.size, size(10, 3));
    assert_eq!(texts(&snapshot.rows), vec!["hey", "", ""]);
}

#[test]
fn resize_reports_in_band_when_the_program_asked_for_it() {
    let mut screen = screen(10, 3);
    screen.feed(b"\x1b[?2048h");
    let mut sink = Recorder::default();
    screen.resize(31, 6, &mut sink);
    assert_eq!(sink.replies(), b"\x1b[48;6;31;96;248t".to_vec());
}

#[test]
fn the_scrollback_limit_drops_old_rows() {
    const LINES: usize = 12_000;
    let mut screen = screen(20, 2);
    let lines: Vec<u8> = (0..LINES).flat_map(|line| format!("{line}\r\n").into_bytes()).collect();
    screen.feed(&lines);
    let kept = screen.snapshot(usize::MAX).scrollback.len();
    // libghostty-vt prunes whole pages, and a page holds thousands of rows this
    // narrow, so it can keep a page less than the limit: the bounds leave room for
    // that, and the upper one is still far below the lines fed.
    let (fewest, most) = (SCROLLBACK_LINES / 5, LINES / 2);
    assert!((fewest..most).contains(&kept), "{kept} rows kept");
}

#[test]
fn the_default_scrollback_keeps_its_lines_whatever_their_bytes() {
    // NOTE: a byte limit cuts the scrollback before the line limit does. In
    // libghostty-vt 1.0.0 a limit of `None` is the library's default of 10,000 bytes,
    // one page, which holds only some hundred rows of this width.
    let mut screen = screen(160, 48);
    let lines: Vec<u8> =
        (0..8000).flat_map(|line| format!("line {line}\r\n").into_bytes()).collect();
    screen.feed(&lines);
    let kept = screen.snapshot(usize::MAX).scrollback.len();
    // libghostty-vt prunes whole pages, so it can keep one page less than the limit.
    assert!(kept >= SCROLLBACK_LINES * 4 / 5, "{kept} rows of scrollback kept");
}

#[test]
fn a_snapshot_round_trip_restores_the_same_screen() {
    let mut screen = screen(10, 3);
    screen
        .feed(b"1\r\n2\r\n3\r\n\x1b[1;31mred\x1b[0m\r\nlast\x1b]2;title\x07\x1b]7;file:///tmp\x07");
    let bytes = screen.encode_snapshot().unwrap();
    let mut restored = restore(&bytes);
    assert_eq!(restored.snapshot(100), screen.snapshot(100));
    assert_eq!(pwd(&restored).as_deref(), Some("file:///tmp"));
    assert_eq!(cursor(&mut restored), cursor(&mut screen));
    // The callbacks are installed again, so the restored screen answers queries.
    assert_eq!(restored.feed(b"\x1b[5n").replies(), b"\x1b[0n".to_vec());
}

#[test]
fn a_full_reset_of_a_restored_screen_reports_its_cleared_title() {
    let mut screen = screen(10, 2);
    screen.feed(b"\x1b]2;old\x07");
    let mut restored = restore(&screen.encode_snapshot().unwrap());
    assert_eq!(restored.feed(b"\x1bc").calls, vec![Call::Title(String::new())]);
}

#[test]
fn a_sequence_cut_by_a_snapshot_completes_after_the_restore() {
    let mut screen = screen(10, 2);
    screen.feed(b"hello\x1b[3");
    let bytes = screen.encode_snapshot().unwrap();
    let mut restored = restore(&bytes);
    restored.feed(b"1mX");
    let row = row(&mut restored, 0);
    assert_eq!(texts(std::slice::from_ref(&row)), vec!["helloX"]);
    assert_eq!(row.cells[5].fg, Some(Color::Indexed(1)));
}

#[test]
fn a_screen_restored_inside_a_sequence_can_make_a_snapshot_at_once() {
    let mut screen = screen(10, 2);
    screen.feed(b"hello\x1b[3");
    let mut restored = restore(&screen.encode_snapshot().unwrap());
    // Still inside the sequence: the restored terminal kept its continuation.
    let mut again = restore(&restored.encode_snapshot().unwrap());
    again.feed(b"1mX");
    let row = row(&mut again, 0);
    assert_eq!(texts(std::slice::from_ref(&row)), vec!["helloX"]);
    assert_eq!(row.cells[5].fg, Some(Color::Indexed(1)));
}

#[test]
fn restore_refuses_bytes_that_are_not_a_whole_snapshot() {
    let err = GhosttyScreen::restore(b"not a snapshot").unwrap_err();
    assert!(matches!(err, GhosttyError::DecodeSnapshot { .. }), "{err:?}");
    let mut screen = screen(10, 2);
    let bytes = screen.encode_snapshot().unwrap();
    let err = GhosttyScreen::restore(&bytes[..bytes.len() / 2]).unwrap_err();
    assert!(matches!(err, GhosttyError::DecodeSnapshot { .. }), "{err:?}");
}

#[test]
fn the_factory_builds_the_screen_on_the_actor_thread() {
    let (handle, mut events) =
        ScreenActor::spawn("screen-ghostty-test", factory(size(8, 2)), size(12, 3)).unwrap();
    handle.feed_blocking(Bytes::from_static(b"hi\x1b[c"), Seq::ZERO).unwrap();
    assert_eq!(
        events.blocking_recv(),
        Some(ScreenEvent::PtyReply(Bytes::from_static(b"\x1b[?62;22c")))
    );
    let capture = handle.snapshot_blocking(0).unwrap();
    assert_eq!(capture.at, Seq::new(5));
    assert_eq!(capture.snapshot.size, size(12, 3));
    assert_eq!(texts(&capture.snapshot.rows), vec!["hi", "", ""]);
    handle.shutdown_blocking().unwrap();
    assert_eq!(events.blocking_recv(), None);
}

#[test]
fn the_marks_and_the_cwd_arrive_once_with_their_offsets() {
    let (handle, mut events) =
        ScreenActor::spawn("screen-ghostty-test", factory(size(20, 4)), size(20, 4)).unwrap();
    // 0..8 A, 8..34 OSC 7, "$ ", 36..44 B, "ls\r\n", 48..56 C, "out\r\n", 61..71 D;2.
    // The chunks cut A in two and OSC 7 between the two bytes of its `ESC \`.
    let stream: &[u8] = b"\x1b]133;A\x07\x1b]7;file://arch/home/egg\x1b\\$ \x1b]133;B\x07ls\r\n\
        \x1b]133;C\x07out\r\n\x1b]133;D;2\x07";
    let mut at = 0;
    for chunk in [&stream[..4], &stream[4..33], &stream[33..58], &stream[58..]] {
        handle.feed_blocking(Bytes::copy_from_slice(chunk), Seq::new(at)).unwrap();
        at += chunk.len() as u64;
    }
    handle.shutdown_blocking().unwrap();
    let mut marks = Vec::new();
    while let Some(event) = events.blocking_recv() {
        if let ScreenEvent::ShellMark(mark) = event {
            marks.push((mark.start.get(), mark.end.get(), mark.kind));
        }
    }
    let offsets: Vec<(u64, u64)> = marks.iter().map(|(start, end, _)| (*start, *end)).collect();
    assert_eq!(offsets, vec![(0, 8), (8, 34), (36, 44), (48, 56), (61, 71)]);
    assert!(matches!(
        &marks[1].2,
        efr_screen::ShellMarkKind::CwdChanged { path, .. } if path.as_os_str() == "/home/egg"
    ));
    assert!(matches!(
        &marks[4].2,
        efr_screen::ShellMarkKind::SemanticPrompt(efr_screen::SemanticPromptEvent::CommandEnd {
            exit_code: Some(2),
            ..
        })
    ));
}

#[test]
fn a_feed_of_the_actor_reaches_the_terminal_in_full_with_its_marks() {
    let mut screen = screen(20, 3);
    screen.feed(b"\x1b]133;A\x07$ \x1b]7;file://arch/srv\x07ok");
    assert_eq!(texts(&screen.snapshot(0).rows), vec!["$ ok", "", ""]);
    assert_eq!(pwd(&screen).as_deref(), Some("file://arch/srv"));
}

#[test]
fn restore_factory_restores_on_the_actor_thread() {
    let mut screen = screen(10, 2);
    screen.feed(b"kept\x1b]2;old\x07");
    let snapshot = screen.encode_snapshot().unwrap();
    let factory = restore_factory(snapshot, size(10, 2));
    let (handle, _events) =
        ScreenActor::spawn("screen-ghostty-test", factory, size(10, 2)).unwrap();
    let capture = handle.snapshot_blocking(0).unwrap();
    assert_eq!(texts(&capture.snapshot.rows), vec!["kept", ""]);
    assert_eq!(capture.snapshot.title.as_deref(), Some("old"));
}

#[test]
fn restore_factory_starts_blank_when_the_snapshot_is_broken() {
    let factory = restore_factory(b"GHOSTSNP broken".to_vec(), size(6, 2));
    let (handle, _events) = ScreenActor::spawn("screen-ghostty-test", factory, size(6, 2)).unwrap();
    let capture = handle.snapshot_blocking(0).unwrap();
    assert_eq!(capture.snapshot.size, size(6, 2));
    assert_eq!(texts(&capture.snapshot.rows), vec!["", ""]);
}

/// A 160x48 screen whose scrollback of 5000 lines is full of build output.
fn full_screen() -> Fed {
    let mut screen = screen(160, 48);
    let lines: Vec<u8> = (0..6000)
        .flat_map(|line| {
            format!("\x1b[32m   Compiling\x1b[0m crate-{line} v0.{line}.0 (/src/crates/crate-{line})\r\n")
                .into_bytes()
        })
        .collect();
    screen.feed(&lines);
    screen
}

/// The resident bytes of both screens of `screen`, as libghostty-vt counts them.
fn resident_bytes(screen: &GhosttyScreen) -> u64 {
    let usage = screen.terminal.memory_usage().unwrap();
    usage.primary_resident_bytes + usage.alternate_resident_bytes
}

/// Runs the idle steps of `screen` until it has no work left, and counts them.
fn idle_until_done(screen: &mut GhosttyScreen) -> usize {
    std::iter::from_fn(|| screen.idle()).count() + 1
}

#[test]
fn the_idle_steps_compress_the_scrollback_and_keep_its_text() {
    let mut screen = full_screen();
    assert_eq!(screen.idle_delay(), Some(super::COMPRESS_AFTER));
    assert!(screen.terminal.memory_usage().unwrap().compression_supported);
    let before = resident_bytes(&screen);
    let text = texts(&screen.snapshot(usize::MAX).scrollback);
    idle_until_done(&mut screen);
    let after = resident_bytes(&screen);
    assert!(after < before / 2, "{before} bytes before the steps, {after} after");
    assert!(screen.terminal.memory_usage().unwrap().primary_compressed_pages > 0);
    // Nothing changed since the last pass, so there is no work.
    assert_eq!(screen.idle(), None);
    // A snapshot without scrollback reads no compressed page.
    screen.snapshot(0);
    assert_eq!(screen.idle(), None);
    // A snapshot of the scrollback decompresses it, and the next pass compresses it
    // again.
    assert_eq!(texts(&screen.snapshot(usize::MAX).scrollback), text);
    assert!(resident_bytes(&screen) > after * 2);
    idle_until_done(&mut screen);
    assert!(resident_bytes(&screen) < before / 2);
}

#[test]
fn new_output_gives_the_idle_steps_work_again() {
    let mut screen = full_screen();
    idle_until_done(&mut screen);
    assert_eq!(screen.idle(), None);
    let done = screen.compressed;
    screen.feed(&b"more\r\n".repeat(500));
    // The output changed the activity, so a new pass runs and ends at the new value.
    assert_ne!(screen.terminal.compression_activity().ok(), done);
    let before = resident_bytes(&screen);
    idle_until_done(&mut screen);
    assert_eq!(screen.compressed, screen.terminal.compression_activity().ok());
    assert!(resident_bytes(&screen) <= before);
    assert_eq!(screen.idle(), None);
}

#[test]
#[ignore = "bench: run by just bench-perf"]
fn bench_perf_idle_compression() {
    let mut report = efr_test_support::bench::Report::new("bench_perf_idle_compression");
    let mut screen = full_screen();
    let rss = || efr_test_support::bench::Memory::of_self().map_or(0, |memory| memory.rss);
    report.bytes("160x48 5000 lines library resident before", resident_bytes(&screen));
    report.bytes("160x48 5000 lines process rss before", rss());
    let started = std::time::Instant::now();
    let steps = idle_until_done(&mut screen);
    let took = started.elapsed();
    report.bytes("160x48 5000 lines library resident after", resident_bytes(&screen));
    report.bytes("160x48 5000 lines process rss after", rss());
    report.count("160x48 5000 lines idle steps", steps as u64);
    report.time("160x48 5000 lines all steps", &[took]);
    let usage = screen.terminal.memory_usage().unwrap();
    report.count("160x48 5000 lines pages", usage.primary_pages);
    report.count("160x48 5000 lines compressed pages", usage.primary_compressed_pages);
    report.finish();
}

/// Output that tests the rules of the text: wraps, blanks, wide characters, graphemes,
/// tabs, erased colours, cursor moves and blank lines around the cursor.
const TEXT_CASES: &[&[u8]] = &[
    b"",
    b"plain\r\nlines\r\n",
    b"trailing   \r\n   leading\r\n\r\n\r\nafter blank lines",
    b"a line longer than the width wraps once and then again and again",
    b"exactly ten",
    b"wrap at a blank  next\r\n",
    b"wide \xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e at the edge\xe6\x97\xa5\xe6\x97\xa5\xe6\x97\xa5",
    b"123456789\xe6\x97\xa5 the wide one starts the next row",
    b"e\xcc\x81 combining, \xf0\x9f\x91\xa8\xe2\x80\x8d\xf0\x9f\x91\xa9 zwj, \xf0\x9f\x87\xa9\xf0\x9f\x87\xaa flag",
    b"tab\tstop\tand\tmore\r\n\ttab first",
    b"     \r\nonly spaces above\r\n          \r\n",
    b"\x1b[41m\x1b[Kred erase\x1b[0m\r\n\x1b[44m   \x1b[0m\r\n",
    b"\x1b[31mcolours\x1b[0m are \x1b[1mdropped\x1b[0m",
    b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n11\r\n12",
    b"top\r\n\r\n\r\n\r\n\x1b[2A",
    b"top\r\n\r\n\r\n\r\n\x1b[3A",
    b"\r\n\r\n\r\n\x1b[1A",
    b"above\x1b[5;1Hbelow the cursor\x1b[2;1H",
    b"a long line that wraps into blank rows          \x1b[3;1H",
    b"done\r\n\x1b[?1049hfull screen\x1b[?1049l",
    b"progress 10%\r progress 50%\rprogress 100%\r\n",
    b"layer a: 10%\r\nlayer b: 0%\r\n\x1b[2Alayer a: done\r\n\x1b[2Klayer b: done\r\n",
    b"a status line that is wider than the screen 10%\r\n\x1b[2A\x1b[2K\x1b[1B\x1b[2K\x1b[1A\
      a status line that is wider than the screen 50%\r\n\x1b[2A\x1b[J\
      a status line that is wider than the screen done\r\n",
    b"layer 1: Downloading [=====>     ] 12MB/50MB\r\nlayer 2: Waiting\r\n\
      \x1b[2A\x1b[2Klayer 1: Pull complete\r\n\x1b[2Klayer 2: Downloading [=>   ]\r\n\
      \x1b[1A\x1b[2Klayer 2: Pull complete\r\n",
    b"\x1b[1G\x1b[0Kspinner one\x1b[1G\x1b[0Kspinner two that is long enough to wrap\
      \x1b[1G\x1b[0Kadded 12 packages\r\n",
    b"line one\r\nline two\r\n\x1b[1A\x1b[1A\x1b[Kreplaced one that is long enough to wrap twice over\r\n",
];

#[test]
fn the_formatter_text_is_the_text_of_the_cells() {
    let mut differ = Vec::new();
    for (index, case) in TEXT_CASES.iter().enumerate() {
        for (cols, rows) in [(10, 4), (40, 6), (3, 2)] {
            let mut screen = screen(cols, rows);
            screen.feed(case);
            let cells = cells_text(&mut *screen, usize::MAX);
            let text = screen.text(usize::MAX);
            if text != cells {
                differ.push(format!("case {index} at {cols}x{rows}: {text:?} != {cells:?}"));
            }
        }
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
}

#[test]
fn the_formatter_text_of_long_output_is_the_text_of_the_cells() {
    let mut screen = screen(80, 24);
    let mut output = Vec::new();
    for line in 0..900 {
        let pad = " ".repeat(line % 7);
        output.extend_from_slice(
            format!(
                "\x1b[1A\r\x1b[2K   Compiling crate-{line}{pad} \u{65e5}\r\n\x1b[2K    Building \
                 [{:=<width$}] {line}/900\r\n",
                "",
                width = line % 90
            )
            .as_bytes(),
        );
    }
    screen.feed(&output);
    assert!(screen.scrollback_rows() > 800);
    let cells = cells_text(&mut *screen, usize::MAX);
    assert_eq!(screen.text(usize::MAX), cells);
}

/// Where the formatter and the cells differ. Each case is the smallest input that a
/// search over 12,000 random inputs found for its kind; the screen and the input
/// are first, then the formatter's text, then the text of the cells.
#[test]
fn the_formatter_and_the_cells_differ_only_at_these_edges() {
    let cases: &[(u16, u16, &str, &str, &str)] = &[
        // A soft-wrapped row that an erase to its start empties keeps its wrap flag.
        // The cells join it to the next row as spaces; the formatter gives an empty
        // line.
        (4, 3, "abcdef\x1b[1;4H\x1b[1K", "\nef", "    ef"),
        // The same for a row that only a tab moved over before a wide character
        // wrapped.
        (9, 2, "\t\u{65e5}", "\n\u{65e5}", "        \u{65e5}"),
        // A wide character that did not fit left an empty last cell, and then the
        // next row was written over. The cells keep the empty cell as a space.
        (
            8,
            1,
            "bc\x1b[Dbcae\u{301}\u{65e5}\u{672c}\rdef",
            "bbcae\u{301}\u{65e5}def",
            "bbcae\u{301}\u{65e5} def",
        ),
        // That empty last cell has a colour: the cells keep it as a space.
        (4, 4, "\x1b[44m\x1b[C\u{65e5}\u{65e5}", " \u{65e5}\u{65e5}", " \u{65e5} \u{65e5}"),
        // A space that the program printed in the last column, before a wide
        // character that starts the next row. The cells cannot tell it from the
        // empty cell above and drop it.
        (3, 2, "ab \u{65e5}", "ab \u{65e5}", "ab\u{65e5}"),
    ];
    for &(cols, rows, input, formatter, cells) in cases {
        let mut screen = screen(cols, rows);
        screen.feed(input.as_bytes());
        let text = |text: Option<ScreenText>| text.map(|text| text.text);
        assert_eq!(text(screen.text(usize::MAX)).as_deref(), Some(formatter), "{input:?}");
        assert_eq!(text(cells_text(&mut *screen, usize::MAX)).as_deref(), Some(cells), "{input:?}");
    }
}

#[test]
fn a_full_scrollback_gives_no_text() {
    let mut screen = screen(10, 2);
    screen.feed(b"1\r\n2\r\n3\r\n4\r\n5");
    assert_eq!(screen.scrollback_rows(), 3);
    assert_eq!(screen.text(3), None);
    assert_eq!(screen.text(4).map(|text| text.text), Some("1\n2\n3\n4\n5".to_owned()));
}

#[test]
fn a_text_read_of_the_scrollback_gives_the_idle_steps_work_again() {
    let mut screen = full_screen();
    idle_until_done(&mut screen);
    assert_eq!(screen.idle(), None);
    assert!(screen.text(usize::MAX).is_some_and(|text| text.text.contains("crate-5999")));
    assert_ne!(screen.idle(), None);
}

#[test]
fn the_alternate_screen_follows_the_mode() {
    let mut screen = screen(10, 2);
    assert!(!screen.alternate_screen());
    screen.feed(b"\x1b[?1049h");
    assert!(screen.alternate_screen());
    screen.feed(b"\x1b[?1049l");
    assert!(!screen.alternate_screen());
}
