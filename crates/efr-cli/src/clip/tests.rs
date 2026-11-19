use std::path::Path;

use pretty_assertions::assert_eq;

use super::*;

/// One packet as Ghostty writes it: `keys`, then `payload` after a `;` when it has one.
fn packet(keys: &str, payload: &str) -> String {
    if payload.is_empty() {
        format!("\x1b]5522;{keys}\x1b\\")
    } else {
        format!("\x1b]5522;{keys};{payload}\x1b\\")
    }
}

/// A paste event of a clipboard with `types`.
fn event(types: &str) -> String {
    let listing = encode(format!("{types}\n").as_bytes());
    [
        packet("type=read:status=OK:pw=secret22", ""),
        packet("type=read:status=DATA:mime=Lg==:pw=secret22", &listing),
        packet("type=read:status=DONE:pw=secret22", ""),
    ]
    .concat()
}

/// A data packet of `mime` with `bytes`.
fn data(mime: &str, bytes: &[u8]) -> String {
    packet(&format!("type=read:status=DATA:mime={}", encode(mime.as_bytes())), &encode(bytes))
}

/// Feeds `stream` to a new reply in pieces of `step` bytes, and gives how it ended.
fn reply(image: bool, limit: u64, stream: &[u8], step: usize) -> (Option<End>, Vec<u8>) {
    let image = image.then_some(("image/png", "png"));
    let mut reply = Reply::new(image, limit);
    let mut packets = Packets::default();
    for piece in stream.chunks(step) {
        let mut other = Vec::new();
        for found in packets.feed(piece, &mut other).unwrap() {
            if let Next::Ended(end) = reply.packet(&found) {
                reply.typed_after(&other);
                return (Some(end), reply.typed);
            }
        }
        if let Next::Ended(end) = reply.keys(&other) {
            return (Some(end), reply.typed);
        }
    }
    (None, reply.typed)
}

#[test]
fn an_event_gives_its_password_its_location_and_its_types() {
    let parsed = Event::parse(&event("image/png text/plain TEXT"));
    assert_eq!(parsed.password, "secret22");
    assert_eq!(parsed.location, None);
    assert_eq!(parsed.image(), Some(("image/png", "png")));
    assert!(parsed.text());

    let primary = packet("type=read:status=OK:loc=primary:pw=p2", "");
    assert_eq!(Event::parse(&primary).location.as_deref(), Some("primary"));
    let denied = Event::parse(&packet("type=read:status=EPERM", ""));
    assert_eq!(denied.error.as_deref(), Some("EPERM"));
}

#[test]
fn the_best_image_type_wins_and_the_read_asks_for_it_and_the_text_at_once() {
    let parsed = Event::parse(&event("image/gif text/plain image/jpeg"));
    assert_eq!(parsed.image(), Some(("image/jpeg", "jpg")));
    let request = parsed.request(&["image/jpeg", TEXT_TYPE]);
    let expected = format!(
        "\x1b]5522;type=read:pw=secret22:name=UGFzdGUgZXZlbnQ=;{}\x1b\\",
        encode(b"image/jpeg text/plain")
    );
    assert_eq!(String::from_utf8(request).unwrap(), expected);
}

#[test]
fn packets_split_in_any_piece_keep_the_keys_around_them() {
    let stream = format!("ab{}cd\x1b]55", data("image/png", b"\x89PNG"));
    for step in 1..stream.len() {
        let mut packets = Packets::default();
        let (mut found, mut other) = (Vec::new(), Vec::new());
        for piece in stream.as_bytes().chunks(step) {
            found.extend(packets.feed(piece, &mut other).unwrap());
        }
        other.extend(packets.rest());
        assert_eq!(found.len(), 1, "step {step}");
        assert_eq!(decode(&found[0].payload).unwrap(), b"\x89PNG");
        assert_eq!(other, b"abcd\x1b]55", "step {step}");
    }
}

#[test]
fn a_packet_that_never_ends_is_broken() {
    let mut packets = Packets::default();
    let long = format!("\x1b]5522;type=read;{}", "A".repeat(PACKET_MAX));
    assert_eq!(packets.feed(long.as_bytes(), &mut Vec::new()), Err(Broken));
}

#[test]
fn an_image_reply_gives_the_image_and_the_keys_typed_during_it() {
    let stream = [
        packet("type=read:status=OK", ""),
        data("image/png", b"\x89PNG"),
        "typed".to_owned(),
        data("image/png", b"rest"),
        data("text/plain", b"also text"),
        packet("type=read:status=DONE", ""),
        "more".to_owned(),
    ]
    .concat();
    let image = Some(End::Image { extension: "png", bytes: b"\x89PNGrest".to_vec() });
    let (end, typed) = reply(true, 100, stream.as_bytes(), stream.len());
    assert_eq!((end, typed), (image.clone(), b"typedmore".to_vec()));
    // NOTE: in pieces, a key after the reply in a later read stays in the terminal.
    let (end, typed) = reply(true, 100, stream.as_bytes(), 7);
    assert_eq!(end, image);
    assert!(typed.starts_with(b"typed"), "{typed:?}");
}

#[test]
fn a_text_reply_gives_the_text_and_one_with_nothing_says_so() {
    let text = [data("text/plain", b"hello"), packet("type=read:status=DONE", "")].concat();
    assert_eq!(reply(true, 100, text.as_bytes(), 64).0, Some(End::Text(b"hello".to_vec())));
    let empty = packet("type=read:status=DONE", "");
    assert_eq!(reply(false, 100, empty.as_bytes(), 64).0, Some(End::Nothing));
}

#[test]
fn an_image_over_the_limit_is_read_to_its_end_and_dropped() {
    let stream = [
        data("image/png", &[1; 60]),
        data("image/png", &[2; 60]),
        data("image/png", &[3; 60]),
        packet("type=read:status=DONE", ""),
    ]
    .concat();
    let (end, _) = reply(true, 100, stream.as_bytes(), 64);
    assert_eq!(end, Some(End::TooLarge { bytes: 180, limit: 100 }));
}

#[test]
fn an_error_status_says_what_failed() {
    let denied = packet("type=read:status=EPERM", "");
    assert_eq!(reply(true, 100, denied.as_bytes(), 64).0, Some(End::Denied));
    let primary = packet("type=read:status=ENOSYS", "");
    assert_eq!(reply(true, 100, primary.as_bytes(), 64).0, Some(End::NoPrimary));
    let io = packet("type=read:status=EIO", "");
    assert_eq!(reply(true, 100, io.as_bytes(), 64).0, Some(End::Failed(STOPPED)));
    let bad = packet("type=read:status=DATA:mime=aW1hZ2UvcG5n", "!!");
    assert_eq!(reply(true, 100, bad.as_bytes(), 64).0, Some(End::Failed(STOPPED)));
}

#[test]
fn ctrl_c_drops_the_rest_and_a_second_one_ends_the_drain() {
    let stream = [data("image/png", b"png"), "\x03".to_owned(), data("image/png", b"x")].concat();
    assert_eq!(reply(true, 100, stream.as_bytes(), 64).0, None, "the drain goes on");
    let twice = [stream, "\x03".to_owned()].concat();
    assert_eq!(reply(true, 100, twice.as_bytes(), 64).0, Some(End::Failed(STOPPED)));
}

#[test]
fn the_deadlines_name_what_happened() {
    let mut reply = Reply::new(Some(("image/png", "png")), 100);
    assert_eq!(reply.wait(Duration::ZERO, false), FIRST_BYTE);
    assert_eq!(reply.wait(Duration::from_secs(5), true), GAP);
    assert_eq!(reply.wait(Duration::from_millis(29_500), true), Duration::from_millis(500));
    assert_eq!(reply.quiet(Duration::from_secs(3), false), End::Failed(NO_FIRST_BYTE));
    assert_eq!(reply.quiet(Duration::from_secs(4), true), End::Failed(STOPPED));
    assert_eq!(reply.quiet(TOTAL, true), End::Failed(TOO_LONG));

    reply.too_long();
    assert_eq!(reply.wait(TOTAL, true), QUIET, "the drain waits for quiet");
    assert_eq!(reply.quiet(TOTAL, true), End::Failed(TOO_LONG));
}

#[test]
fn an_answer_is_its_kind_its_text_and_the_typed_keys_with_no_nul_in_the_text() {
    let answer = Answer { kind: "text", text: b"a\0b".to_vec(), typed: b"xy".to_vec() };
    assert_eq!(answer.bytes(), b"text\0ab\0xy");
    let drained = Answer { kind: "", text: Vec::new(), typed: Vec::new() };
    assert_eq!(drained.bytes(), b"\0\0");
}

/// The clip files of `dir`, by name.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn an_image_and_a_long_text_become_clip_files_of_the_shell() {
    let state = tempfile::tempdir().unwrap();
    let clips = state.path().join(CLIPS_DIR);
    let got = |end| Got { end, typed: b"k".to_vec() };

    let image = End::Image { extension: "png", bytes: b"png".to_vec() };
    let image = answer(got(image), &clips, 42, SystemTime::UNIX_EPOCH);
    let path = std::fs::canonicalize(&clips).unwrap().join("42.1.png");
    assert_eq!(
        (image.kind, image.text.clone(), image.typed),
        ("file", path.clone().into_os_string().into_encoded_bytes(), b"k".to_vec())
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"png");
    let mode = |path: &Path| {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    };
    assert_eq!((mode(&path), mode(&clips)), (0o600, 0o700));

    let long = End::Text(vec![b'x'; TEXT_IN_LINE_MAX + 1]);
    assert_eq!(answer(got(long), &clips, 42, SystemTime::UNIX_EPOCH).kind, "file");
    let short = answer(got(End::Text(b"hi".to_vec())), &clips, 42, SystemTime::UNIX_EPOCH);
    assert_eq!((short.kind, short.text), ("text", b"hi".to_vec()));
    assert_eq!(names(&clips), ["42.1.png", "42.2.txt"]);
}

#[test]
fn a_shell_keeps_at_most_ten_clip_files_and_the_oldest_goes() {
    let state = tempfile::tempdir().unwrap();
    let clips = state.path().join(CLIPS_DIR);
    // NOTE: the files are written now, at about the time of the new dir.
    let now = std::fs::metadata(state.path()).unwrap().modified().unwrap();
    for _ in 0..12 {
        write_clip(&clips, 7, "png", b"x", now).unwrap();
    }
    write_clip(&clips, 8, "png", b"y", now).unwrap();
    let names = names(&clips);
    assert_eq!(names.len(), CLIPS_PER_SHELL + 1);
    assert!(!names.contains(&"7.1.png".to_owned()) && !names.contains(&"7.2.png".to_owned()));
    assert!(names.contains(&"7.12.png".to_owned()) && names.contains(&"8.1.png".to_owned()));

    let next_day = now + CLIP_MAX_AGE + Duration::from_secs(1);
    write_clip(&clips, 9, "png", b"z", next_day).unwrap();
    assert_eq!(self::names(&clips), ["9.1.png"], "a clip of a day ago goes");
}

#[test]
fn a_shell_without_clip_files_starts_at_one() {
    let state = tempfile::tempdir().unwrap();
    let clips = state.path().join(CLIPS_DIR);
    let path = write_clip(&clips, 3, "gif", b"g", SystemTime::UNIX_EPOCH).unwrap();
    assert_eq!(path.file_name().unwrap(), "3.1.gif");
}

#[test]
fn a_size_note_names_the_limit_in_the_number_of_its_key() {
    let state = tempfile::tempdir().unwrap();
    let got = Got { end: End::TooLarge { bytes: 34_000_000, limit: 20 * MIB }, typed: Vec::new() };
    let answer = answer(got, state.path(), 1, SystemTime::UNIX_EPOCH);
    assert_eq!(answer.kind, "error");
    assert_eq!(
        String::from_utf8(answer.text).unwrap(),
        "efr: the clipboard image is 34 MB. The limit is 20 MB (attachments.max_file_mib). \
         Nothing was added."
    );
}
