use efr_store::search::DocKind;
use pretty_assertions::assert_eq;

use super::{
    ANSWER_CAP, COMMAND_CAP, PROMPT_CAP, command_text, cut, index_text, is_stripped, strip,
};

const KEY_BODY: &str = "0123456789abcdefABCDEF0123456789abcd";

/// An ESC inside a key would split it in two words that the redaction does not
/// recognise; the strip comes first, so the key is one word again.
#[test]
fn text_rules_strip_then_redact() {
    let text = format!("my key ghp_\u{1b}{KEY_BODY} works");
    let doc = index_text(DocKind::Prompt, &text).unwrap();
    assert_eq!(doc, "my key [***] works");
    assert!(!doc.contains("0123456789abcdef"));
}

#[test]
fn text_rules_strip_c1_and_bidi() {
    let text = "a\u{1b}[31mred\u{9b}x\u{202e}evil\u{200b}zero\u{feff}bom\u{7f}del\u{0}nul\r\n\tb";
    assert_eq!(strip(text), "a[31mredxevilzerobomdelnul\n\tb");
    assert_eq!(index_text(DocKind::Answer, text).unwrap(), "a[31mredxevilzerobomdelnul\n\tb");
    // The markers of the highlights (DQ17) never reach the index.
    assert_eq!(strip("\u{2}hit\u{3}"), "hit");
}

/// The strip set is the rule of the text that efr-cli shows: a control character
/// other than the line end and the tab, or a format character.
#[test]
fn strip_set_matches_the_cli() {
    let cli = |c: char| (c.is_control() && c != '\n' && c != '\t') || efr_stdx::text::is_format(c);
    for code in 0..=0x10_ffffu32 {
        if let Some(c) = char::from_u32(code) {
            assert_eq!(is_stripped(c), cli(c), "U+{code:04X}");
        }
    }
    for c in [
        '\u{1b}', '\u{7f}', '\u{85}', '\u{9b}', '\u{200b}', '\u{200e}', '\u{202e}', '\u{2066}',
        '\u{feff}',
    ] {
        assert!(is_stripped(c), "{c:?}");
    }
    for c in ['\n', '\t', 'a', 'é', '✓'] {
        assert!(!is_stripped(c), "{c:?}");
    }
}

/// A key that stands across the cap of each kind stays redacted: the redaction runs on
/// the whole text before the cut.
#[test]
fn text_rules_cut_per_kind() {
    for (kind, cap) in [
        (DocKind::Prompt, PROMPT_CAP),
        (DocKind::Steer, PROMPT_CAP),
        (DocKind::Answer, ANSWER_CAP),
        (DocKind::Command, COMMAND_CAP),
    ] {
        let fill = "x".repeat(cap - 10);
        let text = format!("{fill} ghp_{KEY_BODY}");
        let doc = index_text(kind, &text).unwrap();
        assert!(doc.len() <= cap, "{kind:?}");
        assert!(!doc.contains("0123"), "{kind:?}: {}", &doc[doc.len() - 20..]);
        // A cut at a line end before the cap.
        let text = format!("{}\nsecond line\n{}", "a".repeat(cap / 2), "b".repeat(cap));
        assert_eq!(
            index_text(kind, &text).unwrap(),
            format!("{}\nsecond line", "a".repeat(cap / 2))
        );
    }
}

/// The search title is the first line of the first prompt, cut to 80 characters in the
/// store; a key across that cut is redacted before it.
#[test]
fn a_key_across_the_title_cut_stays_redacted() {
    let text = format!("{} sk-ant-api03-{KEY_BODY}", "t".repeat(70));
    let doc = index_text(DocKind::Prompt, &text).unwrap();
    assert_eq!(doc, format!("{} [***]", "t".repeat(70)));
}

#[test]
fn cut_keeps_whole_characters() {
    assert_eq!(cut("ééé", 3), "é");
    assert_eq!(cut("abc", 3), "abc");
    assert_eq!(cut("\nabcdef", 4), "\nabc");
}

#[test]
fn empty_doc_is_skipped() {
    assert_eq!(index_text(DocKind::Prompt, ""), None);
    assert_eq!(index_text(DocKind::Prompt, " \n\t "), None);
    assert_eq!(index_text(DocKind::Answer, "\u{1b}\u{200b}\u{7f}"), None);
    assert_eq!(
        index_text(DocKind::Command, "cat <<EOF\nbody\nEOF").as_deref(),
        Some("cat <<EOF\nEOF")
    );
}

#[test]
fn heredoc_body_is_not_indexed() {
    let command =
        "mkdir -p conf\ncat > .env <<'EOF'\nAPI_KEY: zebrakey123\nOTHER=1\nEOF\nchmod 600 .env";
    let text = command_text(command);
    assert_eq!(text, "mkdir -p conf\ncat > .env <<'EOF'\nEOF\nchmod 600 .env");
    assert!(!text.contains("zebrakey123"));
    let doc = index_text(DocKind::Command, command).unwrap();
    assert!(doc.contains("cat > .env"), "{doc}");
    assert!(!doc.contains("zebrakey123"), "{doc}");
}

#[test]
fn heredoc_forms() {
    assert_eq!(command_text("cat <<EOF\nx\nEOF\ny"), "cat <<EOF\nEOF\ny");
    assert_eq!(command_text("cat <<\"END\" >f\nx\nEND"), "cat <<\"END\" >f\nEND");
    assert_eq!(command_text("cat <<-EOF\n\tx\n\tEOF\ny"), "cat <<-EOF\n\tEOF\ny");
    assert_eq!(command_text("cat << EOF\nx\nEOF"), "cat << EOF\nEOF");
    assert_eq!(command_text("cat <<\\EOF\nx\nEOF"), "cat <<\\EOF\nEOF");
    // With no end word, the body goes to the end.
    assert_eq!(command_text("cat <<EOF\nx\ny"), "cat <<EOF");
    // `<<EOF` without the tab rule needs the word alone on its line.
    assert_eq!(command_text("cat <<EOF\n\tEOF\nx\nEOF\nz"), "cat <<EOF\nEOF\nz");
}

#[test]
fn here_string_keeps_its_line() {
    assert_eq!(command_text("grep x <<< \"$text\"\nls"), "grep x <<< \"$text\"\nls");
    assert_eq!(command_text("tr a b <<<abc"), "tr a b <<<abc");
}

#[test]
fn two_heredocs() {
    let command = "cat <<A <<'B'\nbody a\nA\nbody b\nB\nafter";
    assert_eq!(command_text(command), "cat <<A <<'B'\nA\nB\nafter");
    let command = "cat <<A\none\nA\ncat <<B\ntwo\nB\nend";
    assert_eq!(command_text(command), "cat <<A\nA\ncat <<B\nB\nend");
}
