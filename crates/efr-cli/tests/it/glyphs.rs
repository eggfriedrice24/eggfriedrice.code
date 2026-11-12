//! One table of glyphs (`efr_render::Glyphs`): no file of efr-cli or efr-render writes
//! one of its glyphs as a literal, so a locale that is not UTF-8 gets the ASCII form of
//! every mark. Comments and tests may name a glyph; code reads it from
//! `RenderOptions::glyphs`.
//!
//! The files that still write a glyph are listed in [`NOT_YET`]: the code of today,
//! which the slices of the terminal look move to the table. A slice that moves a file
//! takes it out of the list; the test fails when a listed file has no glyph left, so the
//! list only shrinks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use efr_render::Glyphs;

/// The files that write a glyph of the table as a literal today, from the root of the
/// workspace. Each slice of the terminal look takes out the files it moves to the table.
const NOT_YET: &[&str] = &[
    "crates/efr-cli/src/commands/shell.rs",
    "crates/efr-cli/src/commands/undo.rs",
    "crates/efr-cli/src/follow/view.rs",
    "crates/efr-cli/src/follow/view/call.rs",
    "crates/efr-cli/src/follow/view/input.rs",
    "crates/efr-cli/src/follow/view/left.rs",
    "crates/efr-cli/src/follow/view/newest.rs",
    "crates/efr-cli/src/follow/view/reasoning.rs",
    "crates/efr-cli/src/follow/view/status.rs",
    "crates/efr-cli/src/format.rs",
    "crates/efr-cli/src/format/call.rs",
    "crates/efr-cli/src/format/card.rs",
    "crates/efr-cli/src/format/changes.rs",
    "crates/efr-cli/src/format/jobs.rs",
    "crates/efr-cli/src/format/patch.rs",
    "crates/efr-cli/src/format/resume.rs",
    "crates/efr-cli/src/format/route.rs",
    "crates/efr-cli/src/format/search.rs",
    "crates/efr-render/src/width.rs",
];

/// The file that holds the table.
const TABLE: &str = "crates/efr-render/src/glyph.rs";

#[test]
fn no_file_writes_a_glyph_of_the_table_as_a_literal() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for crate_dir in ["crates/efr-cli/src", "crates/efr-render/src"] {
        for file in sources(&root.join(crate_dir)) {
            let relative = relative(&root, &file);
            if relative == TABLE {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap();
            let lines = glyph_lines(&text);
            if !lines.is_empty() {
                found.insert(relative, lines);
            }
        }
    }
    let unlisted: Vec<String> = found
        .iter()
        .filter(|(file, _)| !NOT_YET.contains(&file.as_str()))
        .map(|(file, lines)| format!("{file}: {}", lines.join(" | ")))
        .collect();
    assert!(
        unlisted.is_empty(),
        "these files write a glyph as a literal; read it from RenderOptions::glyphs:\n{}",
        unlisted.join("\n")
    );
    let done: Vec<&&str> = NOT_YET.iter().filter(|file| !found.contains_key(**file)).collect();
    assert!(done.is_empty(), "these files have no glyph left; take them out of NOT_YET: {done:?}");
}

#[test]
fn the_check_finds_a_glyph_and_its_escape_but_not_a_comment() {
    assert_eq!(glyph_lines("let mark = \"\u{2713}\";\n").len(), 1);
    assert_eq!(glyph_lines("const OK: &str = \"\\u{2713} \";\n").len(), 1);
    assert_eq!(glyph_lines("let dot = '\\u{00B7}';\n").len(), 1);
    assert_eq!(glyph_lines("/// writes `\u{2713} done`\n// \u{2717}\n").len(), 0);
    assert_eq!(glyph_lines("let ok = \"done\";\n").len(), 0);
}

/// The lines of `text` that write a glyph of the table that is not ASCII, as the
/// character or as its `\u{...}` escape; comment lines are left out.
fn glyph_lines(text: &str) -> Vec<String> {
    let glyphs: Vec<char> = Glyphs::UNICODE
        .all()
        .into_iter()
        .flat_map(|(_, glyph)| glyph.chars())
        .filter(|c| !c.is_ascii())
        .collect();
    let escapes: Vec<String> = glyphs.iter().map(|c| format!("{:x}", u32::from(*c))).collect();
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| {
            line.chars().any(|c| glyphs.contains(&c)) || {
                let lower = line.to_ascii_lowercase();
                escapes.iter().any(|hex| has_escape(&lower, hex))
            }
        })
        .map(|line| line.trim().to_owned())
        .collect()
}

/// True when `line` holds `\u{<hex>}`, with any leading zeros.
fn has_escape(line: &str, hex: &str) -> bool {
    line.match_indices("\\u{").any(|(at, start)| {
        let rest = &line[at + start.len()..];
        rest.split_once('}').is_some_and(|(digits, _)| digits.trim_start_matches('0') == hex)
    })
}

/// The `.rs` files under `dir` that are not tests: no `tests.rs`, nothing under a
/// `tests` or `snapshots` directory.
fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if path.is_dir() {
            if name != "tests" && name != "snapshots" {
                out.extend(sources(&path));
            }
        } else if name.ends_with(".rs") && name != "tests.rs" {
            out.push(path);
        }
    }
    out
}

fn relative(root: &Path, file: &Path) -> String {
    let root = root.canonicalize().unwrap();
    let file = file.canonicalize().unwrap();
    file.strip_prefix(&root).unwrap().to_string_lossy().into_owned()
}
