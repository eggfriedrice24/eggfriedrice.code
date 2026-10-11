use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use super::{MAX_LEFT_OUT, Meta};
use crate::capture::{LeftOut, LeftOutReason};

#[test]
fn a_line_of_an_older_commit_reads_with_defaults() {
    let text = "tree 1\n\nefr turn t pre\n\nefr-meta: {\"conversation\":\"c\",\"turn\":\"t\",\
        \"root\":\"/p\",\"shown\":\"\",\"head\":\"ref: refs/heads/main\",\"at\":\"x\",\
        \"hidden\":[\".env\"]}\n";
    let meta = Meta::of_commit(text);
    assert_eq!(meta.shown, "");
    assert_eq!(meta.hidden, [".env"]);
    assert_eq!(meta.phase, "");
    assert!(meta.left_out.is_empty() && meta.touched.is_empty() && !meta.failed);
    assert_eq!(Meta::of_commit("no line here"), Meta::default());
}

#[test]
fn a_line_reads_back_what_it_wrote_with_the_left_out_record_cut() {
    let mut meta = Meta { turn: "t".to_owned(), phase: "pre".to_owned(), ..Meta::default() };
    let many: Vec<LeftOut> = (0..MAX_LEFT_OUT + 5)
        .map(|n| LeftOut { path: format!("big/{n:05}"), reason: LeftOutReason::TooLarge })
        .collect();
    meta.set_left_out(&many);
    meta.set_modes(&BTreeMap::from([(".env".to_owned(), 0o600)]));
    let message = meta.message();
    assert!(message.starts_with("efr turn t pre\n\nefr-meta: {"), "{message}");
    assert!(message.contains("\"reason\":\"too_large\""), "{message}");
    let back = Meta::of_commit(&message);
    assert_eq!(back, meta);
    assert_eq!(back.left_out.len(), MAX_LEFT_OUT);
    assert_eq!(back.left_out_more, 5);
    assert_eq!(back.modes[".env"], 0o600);
}
