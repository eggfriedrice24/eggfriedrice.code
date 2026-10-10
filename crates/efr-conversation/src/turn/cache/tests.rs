use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::{question, size};

#[test]
fn a_size_reads_in_the_largest_unit_with_one_decimal() {
    assert_eq!(size(0), "0 bytes");
    assert_eq!(size(1023), "1023 bytes");
    assert_eq!(size(1536), "1.5 KiB");
    assert_eq!(size(3 * 1024 * 1024 * 1024), "3.0 GiB");
}

#[test]
fn the_question_names_one_path_or_the_count_and_the_first() {
    let one = [PathBuf::from("/home/u/.cache/go-build/ab")];
    assert_eq!(question(&one), "delete /home/u/.cache/go-build/ab in the real tool caches");
    let two = [PathBuf::from("/home/u/.cache/a"), PathBuf::from("/home/u/.cache/b")];
    assert_eq!(question(&two), "delete 2 paths in the real tool caches, such as /home/u/.cache/a");
    let broken = [PathBuf::from("/home/u/.cache/a\nb")];
    assert!(!question(&broken).contains('\n'), "a path cannot add a line");
}
