use pretty_assertions::assert_eq;

use super::{drain, enable, enable_file, hit, hit_for, marker, scan, text, value};

#[test]
fn scan_gives_the_largest_marker_plus_one() {
    assert_eq!(scan(b""), 0);
    assert_eq!(scan(b"no marker here zq12"), 0);
    assert_eq!(scan(b"zq00000"), 1);
    assert_eq!(scan(b"a zq00041 b zq00007 zq0003x"), 42);
}

#[test]
fn marks_go_to_memory_and_to_the_file_once_enabled() {
    // NOTE: the state is global, so this one test turns both places on.
    assert_eq!(marker(b"zq00009"), 0, "off, a marker is not even looked for");
    hit("before");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("marks.log");
    enable();
    enable_file(&path).unwrap();
    hit("one");
    hit_for("two", "detail");
    value("three", 7, 8);
    text("four", b"x zq00002 y");
    let names: Vec<_> =
        drain().iter().map(|mark| (mark.name, mark.detail, mark.a, mark.b)).collect();
    assert_eq!(
        names,
        [("one", "", 0, 0), ("two", "detail", 0, 0), ("three", "", 7, 8), ("four", "", 3, 11)]
    );
    assert!(drain().is_empty(), "a drain takes the marks");
    let lines = std::fs::read_to_string(&path).unwrap();
    let fields: Vec<Vec<&str>> =
        lines.lines().map(|line| line.split(' ').skip(2).collect()).collect();
    assert_eq!(
        fields,
        [
            vec!["one", "0", "0"],
            vec!["two:detail", "0", "0"],
            vec!["three", "7", "8"],
            vec!["four", "3", "11"]
        ]
    );
    assert_eq!(marker(b"zq00009"), 10);
}
