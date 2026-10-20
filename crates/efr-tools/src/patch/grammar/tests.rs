use super::GRAMMAR;

#[test]
fn the_grammar_names_every_marker_of_the_format() {
    for marker in [
        "\"*** Begin Patch\"",
        "\"*** End Patch\"",
        "\"*** Add File: \"",
        "\"*** Delete File: \"",
        "\"*** Update File: \"",
        "\"*** Move to: \"",
        "\"*** End of File\"",
        "\"@@\"",
        "\"@@ \"",
    ] {
        assert!(GRAMMAR.contains(marker), "{marker}");
    }
    assert!(GRAMMAR.starts_with("start: "));
    assert!(GRAMMAR.ends_with("%import common.LF\n"));
}
