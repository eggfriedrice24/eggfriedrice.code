//! `cargo xtask design-frames`: the frames of the terminal design as test fixtures.
//!
//! It reads the frames that the design boards export (`frames/TM-<board>.jsonl` under
//! the design folder, `research/terminal-design` by default, which git ignores) and
//! the design system (`boards/tokens.json`), and writes
//! `crates/efr-test-support/fixtures/design/TM-<board>.jsonl`, which is checked in. The
//! tests read only the fixture: `efr_test_support::design`.
//!
//! What it does to each frame:
//!
//! - It resolves each token of the design system to what a terminal shows: `#rrggbb`
//!   in a truecolor frame, the ANSI slot in a frame of 16 colours, nothing in a
//!   `NO_COLOR` frame. The token `fg` is the terminal's own foreground, so it is left
//!   out. A segment drawn in the colour of the background (`fg: bg`) is reverse video
//!   of its background colour, as an SGR 7 shows it.
//! - It names the depth of each frame and, for a frame that draws another frame of its
//!   board in another depth, that frame (`variant_of`).
//! - It drops the notes of the boards and the frames that lose: today's look, the
//!   structured long turn, the home marks that were not chosen, the removed lines and
//!   the light frames.
//! - It makes the corrections of the build spec (section 2.3): no live row for the
//!   reasoning, the detail at the end of the status row, the diff sign at column 2, the
//!   scope `app`, a blank row between calls and prose, one row for each read in `efr
//!   history`, and mock names only (the repository is public).
//!
//! A row keeps the line breaks of the board, so a test gives mock Markdown with a line
//! break at the end of each prose row.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde_json::{Map, Value as Json, json};

use crate::output;

/// Where the fixtures go, from the root of the repository.
const OUT: &str = "crates/efr-test-support/fixtures/design";

/// The design folder, from the root of the repository, when `--from` is not given.
const DEFAULT_FROM: &str = "research/terminal-design";

/// The ANSI slot of each token of the design system in its terminal theme.
const SLOTS: &[(&str, u8)] = &[
    ("bg_light", 0),
    ("red", 1),
    ("green", 2),
    ("yellow", 3),
    ("blue", 4),
    ("purple", 5),
    ("cyan", 6),
    ("comment", 8),
    ("red_bright", 9),
    ("green_bright", 10),
    ("yellow_bright", 11),
    ("blue_bright", 12),
    ("purple_bright", 13),
    ("cyan_bright", 14),
    ("fg_bright", 15),
];

/// Tokens that are the terminal's own foreground in 16 colours: the text and the
/// punctuation of code (TM-Code: "punctuation in the default foreground").
const DEFAULT_FOREGROUND: &[&str] = &["fg", "fg_dark"];

/// The depth of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Depth {
    TrueColor,
    Ansi16,
    NoColor,
}

impl Depth {
    fn name(self) -> &'static str {
        match self {
            Depth::TrueColor => "truecolor",
            Depth::Ansi16 => "ansi16",
            Depth::NoColor => "nocolor",
        }
    }
}

/// The frames that the fixture leaves out (spec 2.3), by board and number.
fn dropped(board: &str, frame: u64) -> bool {
    match board {
        // Today's look.
        "TM-BeforeAfter" => matches!(frame, 1 | 2),
        // Today's look, quiet without the thought row, and the structured long turn.
        "TM-LongTurn" => matches!(frame, 1 | 2 | 5 | 6),
        // The home marks that were not chosen, and the lines that were removed.
        "TM-Scope" => matches!(frame, 12..=16 | 20),
        // The light frames.
        "TM-Depths" => matches!(frame, 13..=16),
        _ => false,
    }
}

/// The depth of a frame and the frame of its board that it draws in another depth.
pub(crate) fn depth_of(board: &str, frame: u64, caption: Option<&str>) -> (Depth, Option<u64>) {
    match (board, frame) {
        ("TM-Depths", 1..=4) => (Depth::TrueColor, None),
        ("TM-Depths", 5..=8) => (Depth::Ansi16, Some(frame - 4)),
        ("TM-Depths", 9..=12) => (Depth::NoColor, Some(frame - 8)),
        ("TM-Calls", 18) => (Depth::Ansi16, Some(17)),
        ("TM-Calls", 19) => (Depth::NoColor, Some(17)),
        ("TM-Code" | "TM-Diffs", 2) => (Depth::Ansi16, Some(1)),
        ("TM-Code" | "TM-Diffs", 3) => (Depth::NoColor, Some(1)),
        ("TM-Questions", 9) => (Depth::Ansi16, Some(1)),
        ("TM-Questions", 10) => (Depth::NoColor, Some(1)),
        ("TM-Markdown", 2) => (Depth::NoColor, Some(1)),
        _ => {
            let caption = caption.unwrap_or_default();
            if caption.contains("16 colours") {
                (Depth::Ansi16, None)
            } else if caption.contains("NO_COLOR") {
                (Depth::NoColor, None)
            } else {
                (Depth::TrueColor, None)
            }
        }
    }
}

pub(crate) fn run(root: &Path, from: Option<PathBuf>) -> anyhow::Result<bool> {
    let from = from.unwrap_or_else(|| root.join(DEFAULT_FROM));
    let tokens = read_tokens(&from.join("boards/tokens.json"))?;
    let frames_dir = from.join("frames");
    let mut boards: Vec<PathBuf> = std::fs::read_dir(&frames_dir)
        .with_context(|| format!("reading {}", frames_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "jsonl")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("TM-"))
        })
        .collect();
    boards.sort();
    let out = root.join(OUT);
    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    let mut written = 0;
    for path in boards {
        let board = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("a board file has no name")?
            .to_owned();
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut lines = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let mut frame: Json = serde_json::from_str(line)
                .with_context(|| format!("a frame of {board} is not JSON"))?;
            let number = frame["frame"].as_u64().context("a frame has no number")?;
            if dropped(&board, number) {
                continue;
            }
            correct(&board, number, &mut frame)?;
            let resolved = resolve(&board, number, &frame, &tokens)?;
            lines.push(escape_dashes(&serde_json::to_string(&resolved)?));
        }
        if lines.is_empty() {
            continue;
        }
        let target = out.join(format!("{board}.jsonl"));
        std::fs::write(&target, lines.join("\n") + "\n")
            .with_context(|| format!("writing {}", target.display()))?;
        written += 1;
    }
    output::line(&format!("design-frames: wrote {written} boards to {OUT}"));
    Ok(true)
}

/// `json` with each em dash and en dash written as a JSON escape: a frame keeps the
/// board's text, and the file keeps the tidy rule of the repository (no dash but the
/// hyphen-minus).
pub(crate) fn escape_dashes(json: &str) -> String {
    json.replace('\u{2014}', "\\u2014").replace('\u{2013}', "\\u2013")
}

/// The hex of each colour token of the design system, aliases resolved.
fn read_tokens(path: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let json: Json = serde_json::from_str(&text)?;
    let list = json["color"]["tokens"].as_array().context("tokens.json has no color.tokens")?;
    let raw: BTreeMap<String, String> = list
        .iter()
        .filter_map(|token| {
            Some((token["name"].as_str()?.to_owned(), token["value"].as_str()?.to_owned()))
        })
        .collect();
    let mut out = BTreeMap::new();
    for (name, value) in &raw {
        let mut value = value.clone();
        // An alias such as `{fg}` names another token.
        for _ in 0..4 {
            match value.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
                Some(alias) => value = raw.get(alias).cloned().unwrap_or_default(),
                None => break,
            }
        }
        if value.starts_with('#') {
            out.insert(name.clone(), value.to_ascii_lowercase());
        }
    }
    Ok(out)
}

/// The corrections of spec 2.3, on the frame as the board exports it.
pub(crate) fn correct(board: &str, frame: u64, json: &mut Json) -> anyhow::Result<()> {
    let rows = json["rows"].as_array_mut().context("a frame has no rows")?;
    match (board, frame) {
        // No live row for the reasoning (DECISIONS.md: the boards lose).
        ("TM-Live", 3) => {
            rows.remove(0);
        }
        ("TM-Sequence", 3) => {
            rows.remove(1);
        }
        // Today's sticky line.
        ("TM-Scope", 17) => {
            rows.remove(0);
        }
        _ => {}
    }
    // The detail of the status row stands last (DECISIONS.md), after three spaces.
    if matches!((board, frame), ("TM-Live", 3 | 5 | 12 | 14 | 25 | 26) | ("TM-Sequence", 3)) {
        for row in rows.iter_mut() {
            move_detail_last(row);
        }
    }
    // A project of the examples is `app` (DECISIONS.md).
    if matches!((board, frame), ("TM-Live", 8 | 14)) {
        for row in rows.iter_mut() {
            for seg in segs_mut(row) {
                if seg["t"] == "efr" {
                    seg["t"] = json!("app");
                }
            }
        }
    }
    // The diff sign at column 2, as the Diffs board and the Legend draw it (spec 2.2).
    if matches!(
        (board, frame),
        ("TM-Sequence", 8) | ("TM-BeforeAfter", 4) | ("TM-Depths", 1 | 5 | 9)
    ) {
        for row in rows.iter_mut() {
            sign_at_column_two(row);
        }
    }
    // One blank row between a run of calls and prose: the Calls board wins (spec 2.2).
    if (board, frame) == ("TM-Scope", 21) {
        for at in [17, 10, 2] {
            rows.insert(at, json!({"segs": [], "fill": null, "live": false, "dim": false}));
        }
    }
    // `efr history` lists each read, and its last line does not promise the reasoning
    // (spec 3.3).
    if (board, frame) == ("TM-Commands", 4) {
        history_reads(rows)?;
    }
    // Mock names only: the repository is public.
    for row in rows.iter_mut() {
        for seg in segs_mut(row) {
            if let Some(text) = seg["t"].as_str() {
                seg["t"] = json!(mock_names(text));
            }
        }
    }
    Ok(())
}

/// `text` with mock data in place of what names a real person or machine: the user of
/// `[sudo] password for <user>:`, the owner of `<owner>'s iPhone` and a tailnet address
/// (100.x.y.z) become `sam` and `100.64.70.12`.
pub(crate) fn mock_names(text: &str) -> String {
    let mut out = text.to_owned();
    if let Some((before, rest)) = out.split_once("password for ")
        && let Some((_, after)) = rest.split_once(':')
    {
        out = format!("{before}password for sam:{after}");
    }
    if let Some((before, after)) = out.split_once("'s iPhone") {
        let start = before.rfind(' ').map_or(0, |at| at + 1);
        out = format!("{}sam's iPhone{after}", &before[..start]);
    }
    let words: Vec<String> = out
        .split(' ')
        .map(|word| {
            let (address, rest) = word.split_at(
                word.find(|c: char| c != '.' && !c.is_ascii_digit()).unwrap_or(word.len()),
            );
            let parts: Vec<&str> = address.split('.').collect();
            let tailnet = parts.len() == 4
                && parts[0] == "100"
                && parts.iter().all(|part| !part.is_empty() && part.len() <= 3);
            if tailnet { format!("100.64.70.12{rest}") } else { word.to_owned() }
        })
        .collect();
    words.join(" ")
}

fn segs_mut(row: &mut Json) -> impl Iterator<Item = &mut Json> {
    row["segs"].as_array_mut().into_iter().flatten()
}

/// Moves the detail after the state (two spaces, then the detail) to the end of a
/// status row, after three spaces.
pub(crate) fn move_detail_last(row: &mut Json) {
    let Some(segs) = row["segs"].as_array_mut() else { return };
    // Only a status row: it starts with the spinner, or the still dot without motion.
    let spinner = segs.first().and_then(|seg| seg["t"].as_str()).is_some_and(|mark| {
        mark.chars().count() == 1
            && mark.chars().all(|c| ('\u{2800}'..='\u{28ff}').contains(&c) || c == '\u{2022}')
    });
    if !spinner {
        return;
    }
    let Some(at) = segs.iter().position(|seg| seg["t"] == "  ") else { return };
    if at + 1 >= segs.len() {
        return;
    }
    let detail = segs.remove(at + 1);
    segs.remove(at);
    segs.push(json!({"t": "   "}));
    segs.push(detail);
}

/// A diff row whose sign stands at column 1 gets it at column 2.
pub(crate) fn sign_at_column_two(row: &mut Json) {
    let Some(segs) = row["segs"].as_array_mut() else { return };
    let signed = segs.len() > 1
        && segs[0]["t"] == " "
        && (segs[1]["t"] == "+" || segs[1]["t"] == "-")
        && segs[1]["bold"] == true;
    if signed {
        segs[0]["t"] = json!("  ");
    }
}

/// The rows of `efr history` in TM-Commands frame 4: the folded reads become one row
/// each, and the last line names what `--verbose` adds today.
fn history_reads(rows: &mut Vec<Json>) -> anyhow::Result<()> {
    let folded = rows
        .iter()
        .position(|row| {
            row["segs"].as_array().is_some_and(|segs| {
                segs.iter()
                    .any(|seg| seg["t"].as_str().is_some_and(|t| t.starts_with("read 3 files")))
            })
        })
        .context("TM-Commands frame 4 has no folded reads")?;
    let paths = [
        "crates/efr-daemon/src/turn.rs",
        "crates/efr-daemon/src/rpc.rs",
        "crates/efr-daemon/tests/steer.rs",
    ];
    let read_rows: Vec<Json> = paths
        .iter()
        .map(|path| {
            json!({"segs": [
                {"t": "  "},
                {"t": "\u{2713}", "fg": "green"},
                {"t": " "},
                {"t": format!("read {path}"), "fg": "comment"}
            ], "fill": null, "live": false, "dim": false})
        })
        .collect();
    rows.splice(folded..=folded, read_rows);
    let promise =
        "efr history --verbose adds the reasoning, the output tails and the tokens of each call";
    let mut found = false;
    for row in rows.iter_mut() {
        for seg in segs_mut(row) {
            if seg["t"] == promise {
                seg["t"] = json!(
                    "efr history --verbose adds the output tails and the tokens of each turn"
                );
                found = true;
            }
        }
    }
    if !found {
        bail!("TM-Commands frame 4 has no line about --verbose");
    }
    Ok(())
}

/// A frame as the fixture writes it: colours resolved for its depth.
fn resolve(
    board: &str,
    number: u64,
    frame: &Json,
    tokens: &BTreeMap<String, String>,
) -> anyhow::Result<Json> {
    let caption = frame["caption"].as_str();
    let (depth, variant_of) = depth_of(board, number, caption);
    let colour = |token: &str| -> anyhow::Result<Option<Json>> {
        if DEFAULT_FOREGROUND.contains(&token) && depth != Depth::TrueColor || token == "fg" {
            return Ok(None);
        }
        match depth {
            Depth::NoColor => Ok(None),
            Depth::TrueColor => tokens
                .get(token)
                .map(|hex| Some(json!(hex)))
                .with_context(|| format!("{board} {number}: no token {token}")),
            Depth::Ansi16 => SLOTS
                .iter()
                .find(|(name, _)| *name == token)
                .map(|(_, slot)| Some(json!(slot)))
                .with_context(|| format!("{board} {number}: no slot for {token}")),
        }
    };
    let mut rows = Vec::new();
    for row in frame["rows"].as_array().context("a frame has no rows")? {
        let dim_row = row["dim"] == true;
        let mut segs = Vec::new();
        for seg in row["segs"].as_array().into_iter().flatten() {
            let mut out = Map::new();
            out.insert("t".to_owned(), seg["t"].clone());
            let fg = seg["fg"].as_str();
            let bg = seg["bg"].as_str();
            let mut reverse = seg["reverse"] == true;
            let (fg, bg) = if fg == Some("bg") {
                // The terminal's background as the colour of text: reverse video.
                reverse = true;
                (bg, None)
            } else {
                (fg, bg)
            };
            if let Some(fg) = fg.map(colour).transpose()?.flatten() {
                out.insert("fg".to_owned(), fg);
            }
            if let Some(bg) = bg.map(colour).transpose()?.flatten() {
                out.insert("bg".to_owned(), bg);
            }
            for attribute in ["bold", "italic", "underline", "dim", "strike"] {
                if seg[attribute] == true || (attribute == "dim" && dim_row) {
                    out.insert(attribute.to_owned(), json!(true));
                }
            }
            if reverse {
                out.insert("reverse".to_owned(), json!(true));
            }
            segs.push(Json::Object(out));
        }
        let fill = match row["fill"].as_str() {
            Some(token) if depth == Depth::TrueColor => colour(token)?,
            _ => None,
        };
        rows.push(json!({"live": row["live"] == true, "fill": fill, "segs": segs}));
    }
    let fg = tokens.get("fg").cloned().unwrap_or_default();
    Ok(json!({
        "board": board,
        "frame": number,
        "caption": frame["caption"],
        "cols": frame["cols"],
        "depth": depth.name(),
        "variant_of": variant_of,
        "fg": fg,
        "rows": rows,
    }))
}

#[cfg(test)]
mod tests;
