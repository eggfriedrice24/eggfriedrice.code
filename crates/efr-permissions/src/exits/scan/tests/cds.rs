//! The `cd` targets of each segment: the shared list gives each segment the same
//! targets as the copy of each list that v0.0.4 made, on random lines with `cd`s,
//! pipes, subshells, groups and substitutions.

use pretty_assertions::assert_eq;
use proptest::prelude::*;

use super::super::{CdTarget, Depth, MAX_DEPTH, Redirect, cd_target, is_assignment, scan_into};
use super::scan;

/// One segment as its readers see it.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    words: Vec<String>,
    raw: Vec<String>,
    redirects: Vec<Redirect>,
    after_cd: bool,
    cds: Option<Vec<CdTarget>>,
    too_deep: bool,
}

/// The segments of `line` as v0.0.4 gave them: each segment with its own copy of the
/// `cd` targets before it.
fn copied_scan(line: &str) -> Vec<Seen> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    scan_into(&chars, &mut out, Depth { at: 0, max: MAX_DEPTH });
    let mut after_cd = false;
    let mut cds: Option<Vec<CdTarget>> = Some(Vec::new());
    let mut seen = Vec::new();
    for segment in &out {
        seen.push(Seen {
            words: segment.words.clone(),
            raw: segment.raw.clone(),
            redirects: segment.redirects.clone(),
            after_cd,
            cds: cds.clone(),
            too_deep: segment.too_deep,
        });
        let Some(at) = segment.words.iter().position(|word| !is_assignment(word)) else {
            continue;
        };
        let program = segment.words[at].as_str();
        if matches!(program, "cd" | "pushd" | "popd") {
            after_cd = true;
            let target = if program == "popd" { None } else { cd_target(segment, at + 1) };
            cds = cds.zip(target).map(|(mut list, target)| {
                list.push(target);
                list
            });
        }
    }
    seen
}

/// The segments of `line` from today's split, as their readers see them.
fn shared_scan(line: &str) -> Vec<Seen> {
    scan(line)
        .into_iter()
        .map(|segment| Seen {
            cds: segment.cds.as_deref().map(<[CdTarget]>::to_vec),
            words: segment.words,
            raw: segment.raw,
            redirects: segment.redirects,
            after_cd: segment.after_cd,
            too_deep: segment.too_deep,
        })
        .collect()
}

fn command() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "cd /a",
        "cd",
        "cd -",
        "cd +1",
        "cd a b",
        "cd -P ~/b",
        "cd \"$X\"",
        "cd '/c d'",
        "cd /e*",
        "pushd /f",
        "pushd",
        "popd",
        "X=1 cd /g",
        "rm -rf x",
        "ls > out",
        "echo $(cd /h && ls)",
        "echo `cd /i`",
        "(cd /j; ls)",
        "{ cd /k; ls; }",
        "echo $(pwd)",
        "git -C x push",
    ])
}

fn line() -> impl Strategy<Value = String> {
    let joint = prop::sample::select(vec!["; ", " && ", " || ", " | ", "\n", " & "]);
    (command(), prop::collection::vec((joint, command()), 0..12)).prop_map(|(first, rest)| {
        let mut line = first.to_owned();
        for (joint, command) in rest {
            line.push_str(joint);
            line.push_str(command);
        }
        line
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16_384))]

    #[test]
    fn each_segment_sees_the_cds_of_the_copied_lists(line in line()) {
        prop_assert_eq!(shared_scan(&line), copied_scan(&line), "{:?}", line);
    }
}

#[test]
fn a_cd_that_efr_cannot_read_hides_the_targets_of_every_later_segment() {
    let seen = shared_scan("cd /a; cd -; cd /b; ls");
    let cds: Vec<Option<usize>> = seen.iter().map(|seen| seen.cds.as_ref().map(Vec::len)).collect();
    assert_eq!(cds, [Some(0), Some(1), None, None]);
    assert_eq!(seen, copied_scan("cd /a; cd -; cd /b; ls"));
}
