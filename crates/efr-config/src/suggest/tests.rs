use super::{closest_key, edits};

#[test]
fn edits_count_added_lost_changed_and_swapped_letters() {
    assert_eq!(edits("mode", "mode"), 0);
    assert_eq!(edits("mod", "mode"), 1);
    assert_eq!(edits("model", "mode"), 1);
    assert_eq!(edits("mide", "mode"), 1);
    assert_eq!(edits("moed", "mode"), 1);
    assert_eq!(edits("", "abc"), 3);
    assert_eq!(edits("abc", ""), 3);
    assert_eq!(edits("kitten", "sitting"), 3);
}

#[test]
fn a_typo_of_a_key_names_the_key() {
    for (typo, key) in [
        ("permissons.allow_full", "permissions.allow_full"), // spellchecker:disable-line
        ("model.provder", "model.provider"),                 // spellchecker:disable-line
        ("modle.provider", "model.provider"),                // spellchecker:disable-line
        ("model.nmae", "model.name"),                        // spellchecker:disable-line
        ("lgo", "log"),
    ] {
        assert_eq!(closest_key(typo).as_deref(), Some(key), "{typo}");
    }
}

#[test]
fn a_key_of_the_file_or_one_far_from_every_key_names_none() {
    assert_eq!(closest_key("permissions.allow_full"), None);
    assert_eq!(closest_key("nothing"), None);
    assert_eq!(closest_key("model"), None);
    // NOTE: two edits from `log`, but a short word with two edits is often another
    // word.
    assert_eq!(closest_key("lag2"), None);
}
