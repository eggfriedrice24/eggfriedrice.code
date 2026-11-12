use serde_json::json;

use super::{
    Depth, correct, depth_of, escape_dashes, mock_names, move_detail_last, sign_at_column_two,
};

#[test]
fn the_detail_of_a_status_row_moves_to_its_end() {
    let mut row = json!({"segs": [
        {"t": "\u{283c}"}, {"t": " "}, {"t": "thinking"}, {"t": "  "}, {"t": "planning"},
        {"t": "   "}, {"t": "9s"}, {"t": "   "}, {"t": "app"}
    ]});
    move_detail_last(&mut row);
    let text: String =
        row["segs"].as_array().unwrap().iter().map(|seg| seg["t"].as_str().unwrap()).collect();
    assert_eq!(text, "\u{283c} thinking   9s   app   planning");
    // A note in one piece with its spaces moves too, and the time keeps its gap.
    let mut row = json!({"segs": [
        {"t": "\u{283c}"}, {"t": " "}, {"t": "reviewing"}, {"t": "  write ~/x   "}, {"t": "40s"},
        {"t": "   "}, {"t": "app"}
    ]});
    move_detail_last(&mut row);
    let text: String =
        row["segs"].as_array().unwrap().iter().map(|seg| seg["t"].as_str().unwrap()).collect();
    assert_eq!(text, "\u{283c} reviewing   40s   app   write ~/x");
    // A row that is not a status row stays.
    let mut prompt = json!({"segs": [{"t": "~"}, {"t": "  "}, {"t": "x"}]});
    let before = prompt.clone();
    move_detail_last(&mut prompt);
    assert_eq!(prompt, before);
}

#[test]
fn a_diff_sign_at_column_one_moves_to_column_two() {
    let mut row = json!({"segs": [{"t": " "}, {"t": "-", "bold": true}, {"t": " x"}]});
    sign_at_column_two(&mut row);
    assert_eq!(row["segs"][0]["t"], "  ");
    let mut prose = json!({"segs": [{"t": " "}, {"t": "-"}]});
    sign_at_column_two(&mut prose);
    assert_eq!(prose["segs"][0]["t"], " ");
}

#[test]
fn each_depth_and_its_truecolor_frame_are_named() {
    assert_eq!(depth_of("TM-Calls", 18, Some("16 colours")), (Depth::Ansi16, Some(17)));
    assert_eq!(depth_of("TM-Depths", 9, Some("the mid-size turn")), (Depth::NoColor, Some(1)));
    assert_eq!(depth_of("TM-Scope", 9, Some("16 colours")), (Depth::Ansi16, None));
    assert_eq!(depth_of("TM-Scope", 18, Some("NO_COLOR")), (Depth::NoColor, None));
    assert_eq!(depth_of("TM-Live", 3, None), (Depth::TrueColor, None));
}

#[test]
fn a_frame_keeps_mock_names_only() {
    let mut frame = json!({"rows": [{"segs": [{"t": "[sudo] password for someone:"}]}]});
    correct("TM-Live", 22, &mut frame).unwrap();
    assert_eq!(frame["rows"][0]["segs"][0]["t"], "[sudo] password for sam:");
    assert_eq!(
        mock_names("paired: someone's iPhone · since Oct 2"),
        "paired: sam's iPhone · since Oct 2"
    );
    assert_eq!(
        mock_names("listening on 100.1.2.3:7447 (tailscale0)"),
        "listening on 100.64.70.12:7447 (tailscale0)"
    );
    assert_eq!(mock_names("ctx 100.5% of 10.0.0.1"), "ctx 100.5% of 10.0.0.1");
}

#[test]
fn a_dash_of_the_board_is_kept_as_a_json_escape() {
    let line = serde_json::to_string(&json!({"t": "> \u{2014} notes"})).unwrap();
    let escaped = escape_dashes(&line);
    assert_eq!(escaped, r#"{"t":"> \u2014 notes"}"#);
    let back: serde_json::Value = serde_json::from_str(&escaped).unwrap();
    assert_eq!(back["t"], "> \u{2014} notes");
}
