use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use efr_render::{Colour, ColourMode, Palette, RenderOptions, Role};
use pretty_assertions::assert_eq;

use super::{
    FZF_FLOOR, NoPicker, Outcome, PickerLook, Version, exec_word, find_on_path, fzf_args,
    fzf_colours, fzf_query_opts, outcome, parse_version,
};

const ID: &str = "019a9b1c-3d00-7a10-8b20-000000000001";

fn look() -> PickerLook {
    let efr = OsStr::new("/home/user/my bin/it's/efr");
    PickerLook::new(efr, "/dev/pts/7", "16,fg:-1".to_owned()).unwrap()
}

fn args() -> Vec<String> {
    fzf_args(&look()).into_iter().map(|arg| arg.into_string().unwrap()).collect()
}

#[test]
fn fzf_args_are_a_snapshot() {
    insta::assert_snapshot!(args().join("\n").replace('\t', "<TAB>"));
}

#[test]
fn fzf_args_reload_with_the_query_in_the_environment_only() {
    let args = args();
    let reload = "--bind=change:first+reload-sync:exec '/home/user/my bin/it'\\''s/efr' search --picker --tty '/dev/pts/7'";
    assert!(args.iter().any(|arg| arg == reload), "{args:#?}");
    assert!(args.iter().any(|arg| arg == "--bind=load:refresh-preview"), "{args:#?}");
    let preview =
        "--preview=exec '/home/user/my bin/it'\\''s/efr' search --preview {1} --tty '/dev/pts/7'";
    assert!(args.iter().any(|arg| arg == preview), "{args:#?}");
    // The reload is the last action of its own --bind, so nothing can close it.
    assert!(args.iter().filter(|arg| arg.contains("reload")).all(|arg| arg.ends_with('\'')));
    for arg in &args {
        assert!(!arg.contains("{q}"), "the query goes in FZF_QUERY: {arg}");
        for never in ["--no-clear", "--tmux", "--popup", "execute", "--query"] {
            assert!(!arg.contains(never), "{never} in {arg}");
        }
    }
    assert!(args.contains(&"--height=40%".to_owned()));
    assert!(args.contains(&"--expect=ctrl-o,esc".to_owned()));
}

#[test]
fn exec_word_keeps_the_exact_bytes() {
    assert_eq!(exec_word(OsStr::new("/usr/bin/efr")).as_deref(), Some("'/usr/bin/efr'"));
    assert_eq!(
        exec_word(OsStr::new("/home/a b/it's")).as_deref(),
        Some(r"'/home/a b/it'\''s'"),
        "a blank and a quote stay as they are"
    );
    assert_eq!(exec_word(OsStr::new("/opt/ünï/efr")).as_deref(), Some("'/opt/ünï/efr'"));
    assert_eq!(exec_word(OsStr::new("/tmp/$(id)/efr")).as_deref(), Some("'/tmp/$(id)/efr'"));
}

#[test]
fn exec_word_refuses_what_fzf_or_sh_would_change() {
    for word in ["/a/{q}/efr", "/a/{1}", "/a/}", "/a\nb", "/a\x1b[31m", "/a\u{85}b", ""] {
        assert_eq!(exec_word(OsStr::new(word)), None, "{word:?}");
    }
    assert_eq!(exec_word(OsStr::from_bytes(b"/a/\xff/efr")), None, "not UTF-8");
}

#[test]
fn look_refuses_an_unsafe_path_or_terminal() {
    assert!(PickerLook::new(OsStr::new("/a/{q}/efr"), "/dev/pts/7", String::new()).is_none());
    assert!(PickerLook::new(OsStr::new("/usr/bin/efr"), "/dev/{pts}", String::new()).is_none());
}

#[test]
fn fzf_query_opts_quote_the_words_as_one_word() {
    assert_eq!(fzf_query_opts("rsync laptop"), "--query='rsync laptop'");
    assert_eq!(fzf_query_opts("it's  here"), r"--query='it'\''s  here'");
    assert_eq!(fzf_query_opts("a\nb\x1bc"), "--query='a b c'");
    assert_eq!(fzf_query_opts("$HOME #x"), "--query='$HOME #x'");
}

#[test]
fn fzf_colours_follow_the_default_palette() {
    let options = RenderOptions::new(80);
    assert_eq!(
        fzf_colours(&options, false),
        "16,fg:-1,fg+:-1,query:-1,hl:3,hl+:3,prompt:3,pointer:3,marker:2,info:-1,spinner:6,header:6,bg:-1,gutter:-1"
    );
}

#[test]
fn fzf_colours_of_the_eggfriedrice_theme_match_its_fzf_extra() {
    let hex = |text: &str| {
        let byte = |at: usize| u8::from_str_radix(&text[at..at + 2], 16).unwrap();
        Colour::Rgb(byte(1), byte(3), byte(5))
    };
    // The colours of `extras/efr/eggfriedrice.toml` of eggfriedrice.nvim.
    let palette = Palette::new()
        .with(Role::Text, hex("#d8d3c3"))
        .with(Role::Muted, hex("#a8a396"))
        .with(Role::Accent, hex("#ffc940"))
        .with(Role::Success, hex("#60e654"))
        .with(Role::Quote, hex("#8695b7"))
        .with(Role::DiffHunk, hex("#78e2d6"));
    let options =
        RenderOptions::new(80).with_colour(ColourMode::TrueColor).with_palette(palette.clone());
    // The same keys of `extras/fzf/eggfriedrice.sh`, but the background, which efr
    // leaves to the terminal.
    assert_eq!(
        fzf_colours(&options, false),
        "dark,fg:#d8d3c3,fg+:#d8d3c3,query:#d8d3c3,hl:#ffc940,hl+:#ffc940,prompt:#ffc940,pointer:#ffc940,marker:#60e654,info:#8695b7,spinner:#78e2d6,header:#78e2d6,bg:-1,gutter:-1"
    );
    assert!(fzf_colours(&options, true).starts_with("light,fg:#d8d3c3,"));
    // In 16 colours, the nearest of the terminal's colours, as efr's own lines show.
    let sixteen = RenderOptions::new(80).with_palette(palette);
    let colours = fzf_colours(&sixteen, false);
    assert!(colours.starts_with("16,"), "{colours}");
    assert!(!colours.contains('#'), "{colours}");
}

#[test]
fn fzf_colours_without_colour_are_black_and_white() {
    let options = RenderOptions::new(80).with_colour(ColourMode::None);
    assert_eq!(
        fzf_colours(&options, false),
        "bw,fg:-1,fg+:-1,query:-1,hl:-1,hl+:-1,prompt:-1,pointer:-1,marker:-1,info:-1,spinner:-1,header:-1,bg:-1,gutter:-1"
    );
}

#[test]
fn parse_version_reads_what_fzf_prints() {
    let version = |major, minor, patch| Some(Version { major, minor, patch });
    assert_eq!(parse_version("0.73.1 (ce4bef75)\n"), version(0, 73, 1));
    assert_eq!(parse_version("0.44.1 (debian)"), version(0, 44, 1));
    assert_eq!(parse_version("0.46"), version(0, 46, 0));
    assert_eq!(parse_version("0.60.0-dev (devel)"), version(0, 60, 0));
    assert_eq!(parse_version(""), None);
    assert_eq!(parse_version("fzf version"), None);
    assert_eq!(parse_version("7"), None);
    assert!(version(0, 44, 1).unwrap() < FZF_FLOOR);
    assert!(version(0, 46, 0).unwrap() >= FZF_FLOOR);
    assert!(version(0, 73, 1).unwrap() > FZF_FLOOR);
}

#[test]
fn a_reason_that_the_user_may_not_expect_has_a_note() {
    let old = NoPicker::TooOld(Version { major: 0, minor: 44, patch: 1 });
    assert_eq!(old.note().as_deref(), Some("fzf 0.44.1 is older than 0.46, so efr shows the list"));
    assert!(NoPicker::UnsafePath.note().is_some());
    assert!(NoPicker::VersionFailed.note().is_some());
    for silent in [
        NoPicker::NotATerminal,
        NoPicker::DumbTerminal,
        NoPicker::NoTerminalInContext,
        NoPicker::ConfigList,
        NoPicker::NotFound,
        NoPicker::NoSearch,
    ] {
        assert_eq!(silent.note(), None, "{silent:?}");
    }
}

#[test]
fn outcome_reads_the_key_and_the_row() {
    let id = ID.parse().unwrap();
    let row = format!("{ID}\tfix the tests  app  2h  3\n");
    let printed = |key: &str| format!("{key}\n{row}").into_bytes();
    assert_eq!(outcome(Some(0), &printed("")), Outcome::Resume(id));
    assert_eq!(outcome(Some(0), &printed("ctrl-o")), Outcome::Print(id));
    assert_eq!(outcome(Some(0), &printed("esc")), Outcome::Closed);
    assert_eq!(outcome(Some(0), b"\n"), Outcome::Closed, "Enter with no row");
    assert_eq!(outcome(Some(1), b"\n"), Outcome::Closed, "no match");
    assert_eq!(outcome(Some(0), b"\n\tenter resume\n"), Outcome::Closed, "an empty id");
    assert_eq!(outcome(Some(130), b""), Outcome::Interrupted);
    assert_eq!(outcome(Some(2), b""), Outcome::Failed(Some(2)));
    assert_eq!(outcome(None, b""), Outcome::Failed(None), "a signal");
    let tabs = format!("\n{ID}\ta title\twith\ttabs\n");
    assert_eq!(outcome(Some(0), tabs.as_bytes()), Outcome::Resume(id));
    assert_eq!(outcome(Some(0), b"\n\xff\xfe\tbad\n"), Outcome::Closed, "not UTF-8");
    assert_eq!(outcome(Some(0), b"\nnot-an-id\tx\n"), Outcome::Closed);
}

/// A directory with an executable `fzf` in it.
fn with_fzf(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let fzf = dir.join("fzf");
    std::fs::write(&fzf, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&fzf, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

#[test]
fn path_walk_takes_absolute_entries_only() {
    let root = tempfile::tempdir().unwrap();
    let relative = with_fzf(root.path(), "node_modules/.bin");
    let found = with_fzf(root.path(), "bin");
    let executable = |path: &Path| path.is_file();
    let cwd = std::env::current_dir().unwrap();
    let rel = pathdiff(&relative, &cwd);
    let path = format!(":.:{rel}:{}", found.display());
    assert_eq!(find_on_path(OsStr::new(&path), executable), Some(found.join("fzf")));
    let path = format!(":.:{rel}");
    assert_eq!(find_on_path(OsStr::new(&path), executable), None);
    assert_eq!(find_on_path(OsStr::new(""), executable), None);
}

/// `path` relative to `base`, with `..` steps.
fn pathdiff(path: &Path, base: &Path) -> String {
    let ups = base.components().count().saturating_sub(1);
    let mut out = "../".repeat(ups);
    out.push_str(path.strip_prefix("/").unwrap().to_str().unwrap());
    out
}
