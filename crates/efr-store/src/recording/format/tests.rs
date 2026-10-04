use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::*;

fn file_of(chunks: &[(i64, &[u8])]) -> Vec<u8> {
    let mut file = Vec::new();
    for (at, data) in chunks {
        encode(*at, data, &mut file);
    }
    file
}

fn chunk_bytes<'f>(file: &'f [u8], parsed: &Parsed) -> Vec<&'f [u8]> {
    parsed.chunks.iter().map(|chunk| &file[chunk.data.clone()]).collect()
}

#[test]
fn the_header_is_magic_format_length_and_time() {
    assert_eq!(header(0x0102, 5), [b'e', b'f', b'r', 1, 5, 0, 0, 0, 0x02, 0x01, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&header(-1, 0)[8..], &[0xff; 8]);
}

#[test]
fn encoded_chunks_parse_back() {
    let file = file_of(&[(10, b"hello "), (20, b"world")]);

    let parsed = parse(&file).unwrap();

    assert_eq!(parsed.valid_len, file.len());
    assert_eq!(chunk_bytes(&file, &parsed), [&b"hello "[..], &b"world"[..]]);
    assert_eq!(parsed.chunks[1].at_micros, 20);
    assert_eq!(parsed.chunks[1].offset, HEADER_LEN + 6);
    assert_eq!(parsed.stream_len(), 11);
}

#[test]
fn a_large_append_becomes_chunks_of_at_most_the_chunk_size() {
    let data = vec![7; MAX_CHUNK * 2 + 10];
    let mut file = Vec::new();
    encode(0, &data, &mut file);

    let parsed = parse(&file).unwrap();

    let lengths: Vec<usize> = parsed.chunks.iter().map(|chunk| chunk.data.len()).collect();
    assert_eq!(lengths, [MAX_CHUNK, MAX_CHUNK, 10]);
}

#[test]
fn every_cut_of_a_file_is_a_torn_tail_after_its_whole_chunks() {
    let file = file_of(&[(1, b"abc"), (2, b"defgh")]);
    let second = HEADER_LEN + 3;
    for cut in 0..=file.len() {
        let parsed = parse(&file[..cut]).unwrap();
        let (whole, valid_len) = match cut {
            _ if cut == file.len() => (2, file.len()),
            _ if cut >= second => (1, second),
            _ => (0, 0),
        };
        assert_eq!(parsed.chunks.len(), whole, "cut at {cut}");
        assert_eq!(parsed.valid_len, valid_len, "cut at {cut}");
    }
}

#[test]
fn zeros_after_the_last_chunk_are_a_torn_tail() {
    let mut file = file_of(&[(1, b"abc")]);
    let valid = file.len();
    file.extend_from_slice(&[0; 40]);

    let parsed = parse(&file).unwrap();

    assert_eq!(parsed.valid_len, valid);
    assert_eq!(parsed.chunks.len(), 1);
}

#[test]
fn garbage_where_a_header_belongs_is_corruption_at_its_offset() {
    let mut file = file_of(&[(1, b"abc")]);
    let offset = file.len();
    file.extend_from_slice(b"this is not a chunk header");
    assert_eq!(parse(&file), Err(offset));

    let mut short = file_of(&[(1, b"abc")]);
    short.extend_from_slice(b"xyz");
    assert_eq!(parse(&short), Err(offset));
}

#[test]
fn fits_keeps_a_segment_within_its_limit() {
    let chunk_cost = (HEADER_LEN + 10) as u64;
    // An empty segment always takes a chunk, even past a tiny limit.
    assert_eq!(fits(0, 5, 10), 10);
    // Room for exactly one more chunk.
    assert_eq!(fits(100, 100 + chunk_cost, 10), 10);
    // One byte short of that: rotate first.
    assert_eq!(fits(100, 100 + chunk_cost - 1, 10), 0);
    // Whole chunks only: the second 64 KiB chunk does not fit.
    let limit = (2 * HEADER_LEN + MAX_CHUNK + 100) as u64;
    assert_eq!(fits(1, limit, MAX_CHUNK * 3), MAX_CHUNK);
}

#[test]
fn clip_returns_the_overlap_with_its_offset() {
    assert_eq!(clip(10, b"abcdef", 0, 100), Some((10, &b"abcdef"[..])));
    assert_eq!(clip(10, b"abcdef", 12, 14), Some((12, &b"cd"[..])));
    assert_eq!(clip(10, b"abcdef", 16, 20), None);
    assert_eq!(clip(10, b"abcdef", 0, 10), None);
    assert_eq!(clip(10, b"abcdef", 14, 12), None);
}

/// The writer's rotation, without files: the segments that a series of appends
/// produces under `limit`, as `(start_seq, file bytes)`.
fn segments_for(appends: &[Vec<u8>], limit: u64) -> Vec<(u64, Vec<u8>)> {
    let mut segments: Vec<(u64, Vec<u8>)> = vec![(0, Vec::new())];
    let mut end = 0u64;
    for (at, data) in appends.iter().enumerate() {
        let mut offset = 0;
        while offset < data.len() {
            let (_, file) = segments.last_mut().unwrap();
            let take = fits(file.len() as u64, limit, data.len() - offset);
            if take == 0 {
                segments.push((end, Vec::new()));
                continue;
            }
            encode(at as i64, &data[offset..offset + take], file);
            end += take as u64;
            offset += take;
        }
    }
    segments
}

proptest! {
    #[test]
    fn any_range_of_any_rotation_reads_back_the_stream_slice(
        appends in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..300), 0..20),
        limit in 17u64..400,
        start in 0u64..7000,
        len in 0u64..7000,
    ) {
        let stream: Vec<u8> = appends.concat();
        let end = start + len;
        let segments = segments_for(&appends, limit);

        let mut read = Vec::new();
        let mut expected_seq = None;
        for (start_seq, file) in &segments {
            prop_assert!(file.len() as u64 <= limit.max((HEADER_LEN + MAX_CHUNK) as u64));
            let parsed = parse(file).unwrap();
            prop_assert_eq!(parsed.valid_len, file.len());
            let mut seq = *start_seq;
            for chunk in &parsed.chunks {
                let data = &file[chunk.data.clone()];
                if let Some((from, part)) = clip(seq, data, start, end) {
                    if let Some(expected) = expected_seq {
                        prop_assert_eq!(from, expected, "clipped chunks are contiguous");
                    }
                    expected_seq = Some(from + part.len() as u64);
                    read.extend_from_slice(part);
                }
                seq += data.len() as u64;
            }
        }

        let total = stream.len() as u64;
        let from = start.min(total) as usize;
        let to = end.min(total) as usize;
        prop_assert_eq!(read, stream[from..to].to_vec());
    }
}
