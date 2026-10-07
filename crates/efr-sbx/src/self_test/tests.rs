use super::*;

#[test]
fn a_check_without_its_argument_is_skipped() {
    let args = SelfTestArgs::default();
    for name in ["write_inside", "write_outside", "mask_empty", "unix_socket", "tcp", "cache_write"]
    {
        assert_eq!(run(name, &args), None, "{name}");
    }
}

#[test]
fn an_unknown_check_is_skipped() {
    assert_eq!(run("no_such_check", &SelfTestArgs::default()), None);
}

#[test]
fn write_inside_passes_in_a_writable_dir() {
    let dir = tempfile::tempdir().unwrap();
    let args = SelfTestArgs { project: Some(dir.path().to_path_buf()), ..SelfTestArgs::default() };
    let (ok, _) = run("write_inside", &args).unwrap();
    assert!(ok);
}

#[test]
fn write_outside_fails_where_the_write_works() {
    // Outside a sandbox the write works, so the check must report a leak.
    let dir = tempfile::tempdir().unwrap();
    let args = SelfTestArgs { outside: Some(dir.path().join("file")), ..SelfTestArgs::default() };
    let (ok, _) = run("write_outside", &args).unwrap();
    assert!(!ok);
}

#[test]
fn check_lines_round_trip() {
    let line = CheckLine { name: "tcp".to_owned(), ok: true, detail: "Err".to_owned() };
    let text = serde_json::to_string(&line).unwrap();
    assert_eq!(serde_json::from_str::<CheckLine>(&text).unwrap(), line);
}
