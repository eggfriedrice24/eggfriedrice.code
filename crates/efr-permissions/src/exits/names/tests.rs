//! The forms that name an exit, and the one check of the user's messages.

use std::path::Path;

use pretty_assertions::assert_eq;
use rstest::rstest;

use super::{names_match, path_forms, word_forms};

fn forms(list: &[&str]) -> Vec<String> {
    list.iter().map(|form| (*form).to_owned()).collect()
}

fn named(names: &[&str], message: &str) -> bool {
    names_match(&forms(names), &[message.to_owned()])
}

#[rstest]
#[case::word("zshrc", "add an alias to my zshrc", true)]
#[case::case("zshrc", "fix My ZSHRC please", true)]
#[case::end_of_text("zshrc", "edit zshrc", true)]
#[case::punctuation("zshrc", "my zshrc, please", true)]
#[case::longer_word("zshrc", "my zshrcx file", false)]
#[case::inside_a_word("push", "pushover", false)]
#[case::underscore("push", "push_it", false)]
#[case::host("github.com", "push it to github.com/me/x", true)]
#[case::dotted_name("hyprland.conf", "change hyprland.conf", true)]
#[case::stem_before_a_dot("hyprland", "open hyprland.conf", true)]
#[case::phrase("sway config", "change my sway config", true)]
#[case::phrase_possessive("sway config", "change sway's config", true)]
#[case::phrase_curly_possessive("sway config", "change sway\u{2019}s config", true)]
#[case::phrase_spaces("sway config", "the Sway   config", true)]
#[case::phrase_word_between("sway config", "my sway window config", false)]
#[case::phrase_order("sway config", "config of sway", false)]
#[case::phrase_glued("sway config", "swayconfig", false)]
#[case::phrase_longer_last_word("git config", "my git configs", false)]
#[case::path_text("~/.zshrc", "look at ~/.zshrc now", true)]
#[case::path_case("/home/u/.zshrc", "look at /HOME/U/.zshrc", false)]
#[case::last_two("u/.zshrc", "edit u/.zshrc", true)]
fn names_match_whole_words(#[case] form: &str, #[case] message: &str, #[case] expected: bool) {
    assert_eq!(named(&[form], message), expected, "{form:?} in {message:?}");
}

#[test]
fn names_match_any_form_in_any_message() {
    let names = forms(&["publish", "registry.example"]);
    let messages = forms(&["build it", "then publish the crate"]);
    assert!(names_match(&names, &messages));
    assert!(!names_match(&names, &forms(&["build it"])));
    assert!(!names_match(&[], &messages));
    assert!(!names_match(&names, &[]));
    assert!(!names_match(&forms(&[""]), &messages));
}

#[test]
fn path_forms_are_the_absolute_tilde_and_last_two_forms() {
    let home = Path::new("/home/u");
    assert_eq!(
        path_forms(Path::new("/home/u/.zshrc"), home),
        ["/home/u/.zshrc", "~/.zshrc", "u/.zshrc"]
    );
    assert_eq!(path_forms(Path::new("/etc/hosts"), home), ["/etc/hosts", "etc/hosts"]);
    assert_eq!(path_forms(Path::new("/home/u"), home), ["/home/u", "home/u"]);
    assert_eq!(path_forms(Path::new("/etc"), home), ["/etc"]);
}

#[rstest]
#[case::dot_file("/home/u/.zshrc", &["zshrc"])]
#[case::stem("/home/u/.config/hypr/hyprland.conf", &["hyprland.conf", "hyprland"])]
#[case::unit("/home/u/.config/systemd/user/syncthing.service", &["syncthing.service", "syncthing"])]
#[case::generic_config("/home/u/.config/sway/config", &[])]
#[case::generic_with_suffix("/home/u/.cargo/config.toml", &[])]
#[case::generic_fish("/home/u/.config/fish/config.fish", &[])]
#[case::generic_init("/home/u/.config/nvim/init.lua", &[])]
#[case::generic_profile("/home/u/.profile", &[])]
#[case::gitconfig("/home/u/.gitconfig", &["gitconfig"])]
fn word_forms_leave_out_generic_stems(#[case] path: &str, #[case] expected: &[&str]) {
    assert_eq!(word_forms(Path::new(path)), forms(expected), "{path}");
}
