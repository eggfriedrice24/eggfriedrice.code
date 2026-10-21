//! The guard of full mode, on `critical_deletes` alone. No line here runs: each one is
//! text that the guard reads.

use std::path::{Path, PathBuf};

use efr_protocol::ProjectId;
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use rstest::rstest;

use super::critical_deletes;
use crate::exits::{Approver, Envelope, ExitInput};
use crate::network::HostList;
use crate::{AutoSupport, CriticalDelete, CriticalTarget, CriticalWhy, Locations};

const HOME: &str = "/home/u";
/// The turn's project, where a line runs unless a test says otherwise.
const APP: &str = "/home/u/p/app";
/// Another registered project.
const LIB: &str = "/home/u/p/lib";
const DATA: &str = "/home/u/.local/share/efr";
const SCRATCH: &str = "/home/u/.local/share/efr/scratch/2026-10-20-a-00000000";

fn id(n: u8) -> ProjectId {
    format!("0192f0c1-7a00-7000-8000-0000000000{n:02}").parse().unwrap()
}

/// A machine with two projects in `~/p`, efr's config in `~/.config/efr`, efr's data
/// in `~/.local/share/efr`, the private tmp dirs and two cache overlays.
fn machine() -> Locations {
    Locations::new(HOME)
        .unwrap()
        .with_sealed_root(format!("{DATA}/secrets"))
        .unwrap()
        .with_write_sealed_root("/home/u/.config/efr")
        .unwrap()
        .with_efr_root(DATA, [PathBuf::from(DATA).join("scratch")])
        .unwrap()
        .with_project(id(1), APP)
        .unwrap()
        .with_project(id(2), LIB)
        .unwrap()
        .with_private_root("/tmp")
        .unwrap()
        .with_private_root("/var/tmp")
        .unwrap()
        .with_cache_root("/home/u/.cache")
        .unwrap()
        .with_cache_root("/home/u/.cargo")
        .unwrap()
}

/// The guarded deletes of `line`, which runs in `dir` (`None`: a dir efr does not
/// know) in a turn of the project [`APP`].
fn found_in(line: &str, dir: Option<&str>) -> Vec<CriticalDelete> {
    let locations = machine();
    let support = AutoSupport::default();
    let hosts = HostList::default();
    let input = ExitInput {
        line,
        command_dir: dir.map(Path::new),
        paths: &[],
        network: false,
        needs: None,
        facts: None,
        locations: &locations,
        project_root: Some(Path::new(APP)),
        scratch: Some(Path::new(SCRATCH)),
        support: &support,
        hosts: &hosts,
    };
    critical_deletes(&input)
}

/// The targets of [`found_in`].
fn targets_in(line: &str, dir: Option<&str>) -> Vec<CriticalTarget> {
    found_in(line, dir).into_iter().map(|delete| delete.target).collect()
}

/// The targets of `line` in the project root.
fn targets(line: &str) -> Vec<CriticalTarget> {
    targets_in(line, Some(APP))
}

fn path(path: &str, why: CriticalWhy) -> CriticalTarget {
    CriticalTarget::Path { path: PathBuf::from(path), why }
}

fn unreadable(word: &str) -> CriticalTarget {
    CriticalTarget::Unreadable { word: word.to_owned() }
}

#[rstest]
#[case::tilde("rm -rf ~")]
#[case::tilde_slash("rm -rf ~/")]
#[case::dollar_home("rm -rf $HOME")]
#[case::braced_home_in_quotes("rm -rf \"${HOME}\"")]
#[case::quoted_home("rm -fr \"$HOME\"")]
#[case::long_option("rm --recursive --force -- ~")]
#[case::home_contents("rm -rf ~/*")]
#[case::absolute("rm -Rf /home/u")]
fn critical_rm_rf_home(#[case] line: &str) {
    assert_eq!(targets(line), [path(HOME, CriticalWhy::Home)], "{line:?}");
}

#[test]
fn critical_rm_rf_home_names_the_part_and_the_home_for_the_user() {
    let found = found_in("ls && rm -rf ~ ", Some(APP));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].part, "rm -rf ~");
    assert_eq!(found[0].describe(Path::new(HOME)), "~ (your home directory)");
}

#[rstest]
#[case::root("rm -rf /")]
#[case::root_contents("rm -rf /*")]
#[case::dotdot("rm -rf /home/u/../..")]
fn critical_rm_rf_root(#[case] line: &str) {
    assert_eq!(targets(line), [path("/", CriticalWhy::Root)], "{line:?}");
}

#[rstest]
#[case::etc("rm -rf /etc", Some("/etc"))]
#[case::usr_lib("rm -rf /usr/lib", Some("/usr/lib"))]
#[case::opt_child("rm -rf /opt/app", Some("/opt/app"))]
#[case::tmp_itself("rm -rf /tmp", Some("/tmp"))]
#[case::user_disks("rm -rf /run/media/u", Some("/run/media/u"))]
#[case::user_disk("rm -rf /run/media/u/disk", Some("/run/media/u/disk"))]
#[case::media_disk("rm -rf /media/u/disk", Some("/media/u/disk"))]
#[case::mnt_disk("rm -rf /mnt/backup", Some("/mnt/backup"))]
#[case::tmp_child("rm -rf /tmp/x", None)]
#[case::var_tmp_child("rm -rf /var/tmp/x", None)]
#[case::deep_usr("rm -rf /usr/lib/python3/x", None)]
#[case::in_a_disk("rm -rf /run/media/u/disk/old", None)]
fn critical_system_dirs(#[case] line: &str, #[case] dir: Option<&str>) {
    let expected: Vec<CriticalTarget> =
        dir.map(|dir| path(dir, CriticalWhy::SystemDir)).into_iter().collect();
    assert_eq!(targets(line), expected, "{line:?}");
}

#[test]
fn critical_dot_config_holds_efr_config() {
    assert_eq!(targets("rm -rf ~/.config"), [path("/home/u/.config", CriticalWhy::HoldsEfr)]);
    assert_eq!(
        targets("rm -rf ~/.local/share"),
        [path("/home/u/.local/share", CriticalWhy::HoldsEfr)]
    );
    // A secret, a floor: the other things a dir can hold.
    assert_eq!(targets("rm -rf ~/.ssh"), [path("/home/u/.ssh", CriticalWhy::HoldsSecret)]);
    assert_eq!(targets("rm -rf ~/.cargo"), [path("/home/u/.cargo", CriticalWhy::HoldsSecret)]);
    assert_eq!(
        targets("rm -rf ~/.config/systemd"),
        [path("/home/u/.config/systemd", CriticalWhy::HoldsFloor)]
    );
}

#[test]
fn critical_dir_with_two_projects() {
    assert_eq!(targets("rm -rf ~/p"), [path("/home/u/p", CriticalWhy::HoldsProjects)]);
    assert_eq!(targets("rm -rf .."), [path("/home/u/p", CriticalWhy::HoldsProjects)]);
}

#[test]
fn critical_project_root() {
    assert_eq!(targets("rm -rf ."), [path(APP, CriticalWhy::ProjectRoot)]);
    assert_eq!(targets("rm -rf ../app"), [path(APP, CriticalWhy::ProjectRoot)]);
    assert_eq!(targets("rm -rf ~/p/lib"), [path(LIB, CriticalWhy::ProjectRoot)]);
    assert_eq!(targets("rm -rf ~/p/lib/*"), [path(LIB, CriticalWhy::ProjectRoot)]);
    assert_eq!(
        found_in("rm -rf ~/p/lib", Some(APP))[0].describe(Path::new(HOME)),
        "~/p/lib (a project)"
    );
}

#[rstest]
#[case::pip("rm -rf ~/.cache/pip")]
#[case::cargo_registry("rm -rf ~/.cargo/registry")]
#[case::cache_glob("rm -rf ~/.cache/pip/*")]
fn not_critical_cache_child(#[case] line: &str) {
    assert_eq!(targets(line), [], "{line:?}");
}

#[rstest]
#[case::downloads_child("rm -rf ~/Downloads/x")]
#[case::project_child("rm -rf build node_modules")]
#[case::other_project_child("rm -rf ~/p/lib/target")]
#[case::scratch_child("rm -rf /home/u/.local/share/efr/scratch/2026-10-20-a-00000000/out")]
fn not_critical_downloads_child(#[case] line: &str) {
    assert_eq!(targets(line), [], "{line:?}");
}

#[rstest]
#[case::quoted_variable_glob("rm -rf \"$X\"/*", "\"$X\"/*")]
#[case::substitution("rm -rf $(cat l)", "$(cat l)")]
#[case::assigned_variable("D=$(pwd); rm -rf \"$D\"", "\"$D\"")]
#[case::bare_glob("rm -rf *", "*")]
#[case::dot_glob("rm -rf .*", ".*")]
#[case::dot_slash_glob("rm -rf ./*", "./*")]
#[case::braced_variable("rm -rf ${X}/build", "${X}/build")]
#[case::other_user("rm -rf ~root", "~root")]
#[case::home_prefix_of_a_longer_name("rm -rf $HOMEDIR", "$HOMEDIR")]
fn critical_unreadable_targets(#[case] line: &str, #[case] word: &str) {
    assert_eq!(targets(line), [unreadable(word)], "{line:?}");
}

#[test]
fn critical_unreadable_targets_name_the_word_for_the_user() {
    let found = found_in("rm -rf \"$X\"/*", Some(APP));
    assert_eq!(found[0].describe(Path::new(HOME)), "\"$X\"/* (efr cannot read the target)");
    assert_eq!(found[0].part, "rm -rf \"$X\"/*");
}

#[test]
fn critical_relative_target_where_the_dir_is_not_known() {
    assert_eq!(targets_in("rm -rf build", None), [unreadable("build")]);
    assert_eq!(targets_in("rm -rf /etc", None), [path("/etc", CriticalWhy::SystemDir)]);
    assert_eq!(targets_in("rm -rf ~/Downloads/x", None), []);
}

#[rstest]
#[case::build("rm -rf build/*")]
#[case::objects("rm -rf src/*.o")]
#[case::nested("rm -rf target/*/incremental")]
#[case::home_child("rm -rf ~/Downloads/*")]
fn not_critical_glob_with_plain_prefix(#[case] line: &str) {
    assert_eq!(targets(line), [], "{line:?}");
}

#[rstest]
#[case::objects("rm *.o")]
#[case::logs("rm -f *.log")]
#[case::in_a_dir("rm -f build/*.d")]
fn not_critical_plain_glob_in_cwd(#[case] line: &str) {
    assert_eq!(targets(line), [], "{line:?}");
}

#[test]
fn critical_plain_glob_in_home() {
    assert_eq!(targets_in("rm -f *", Some(HOME)), [path(HOME, CriticalWhy::Home)]);
    assert_eq!(targets("rm -f ~/*"), [path(HOME, CriticalWhy::Home)]);
    assert_eq!(targets("rm /etc/*.conf"), [path("/etc", CriticalWhy::SystemDir)]);
    assert_eq!(targets_in("rm -f *", None), [unreadable("*")]);
    // A plain `rm` with no pattern is no delete form.
    assert_eq!(targets_in("rm -f notes.txt", Some(HOME)), []);
}

#[rstest]
#[case::copy("cp notes.txt ~")]
#[case::copy_into_home_dir("cp -r build ~/")]
#[case::touch("touch ~")]
#[case::make_dir("mkdir -p ~/x")]
#[case::link("ln -s x ~")]
#[case::move_into_home("mv notes.txt ~")]
#[case::non_recursive_chmod("chmod 700 ~")]
fn not_critical_cp_into_home(#[case] line: &str) {
    assert_eq!(targets(line), [], "{line:?}");
}

#[rstest]
#[case::home("mv ~ /tmp/old-home", path(HOME, CriticalWhy::Home))]
#[case::config("mv ~/.config ~/.config.bak", path("/home/u/.config", CriticalWhy::HoldsEfr))]
#[case::target_dir("mv -t /tmp ~/.ssh", path("/home/u/.ssh", CriticalWhy::HoldsSecret))]
#[case::project("mv . ../old", path(APP, CriticalWhy::ProjectRoot))]
fn critical_mv_home_away(#[case] line: &str, #[case] expected: CriticalTarget) {
    assert_eq!(targets(line), [expected], "{line:?}");
}

#[rstest]
#[case::chmod("chmod -R 700 ~")]
#[case::chmod_dash_mode("chmod -R -w ~")]
#[case::chmod_cluster("chmod -Rv go-rwx ~/")]
#[case::chown("chown -R nobody:nobody ~")]
#[case::chgrp("chgrp --recursive users ~")]
#[case::setfacl("setfacl -Rm u:x:rwx ~")]
fn critical_chmod_r_home(#[case] line: &str) {
    assert_eq!(targets(line), [path(HOME, CriticalWhy::Home)], "{line:?}");
}

#[test]
fn critical_rsync_delete_into_home() {
    assert_eq!(targets("rsync -a --delete backup/ ~/"), [path(HOME, CriticalWhy::Home)]);
    assert_eq!(
        targets("rsync -av --delete-after --exclude .git src/ /etc"),
        [path("/etc", CriticalWhy::SystemDir)]
    );
    // No delete, or a destination on another host.
    assert_eq!(targets("rsync -a backup/ ~/"), []);
    assert_eq!(targets("rsync -a --delete ~/ host:backup/"), []);
    assert_eq!(targets("rsync -a --delete src/ build/"), []);
}

#[rstest]
#[case::delete("find ~ -name '*.tmp' -delete", path(HOME, CriticalWhy::Home))]
#[case::exec_rm("find ~ -type d -exec rm -rf {} +", path(HOME, CriticalWhy::Home))]
#[case::exec_plain_rm("find -L ~/ -type f -exec rm {} \\;", path(HOME, CriticalWhy::Home))]
#[case::no_root("find -delete", path(APP, CriticalWhy::ProjectRoot))]
fn critical_find_home_delete(#[case] line: &str, #[case] expected: CriticalTarget) {
    assert_eq!(targets(line), [expected], "{line:?}");
}

#[test]
fn not_critical_find_that_deletes_below_a_plain_root_or_prints() {
    assert_eq!(targets("find build -name '*.o' -delete"), []);
    assert_eq!(targets("find ~ -name '*.tmp'"), []);
    assert_eq!(targets("find ~ -name x -exec cat {} +"), []);
}

#[test]
fn critical_git_clean_in_home() {
    assert_eq!(targets_in("git clean -fdx", Some(HOME)), [path(HOME, CriticalWhy::Home)]);
    assert_eq!(targets("git -C ~ clean -fd"), [path(HOME, CriticalWhy::Home)]);
    assert_eq!(targets("git -C ../lib clean -f -d"), [path(LIB, CriticalWhy::ProjectRoot)]);
    assert_eq!(targets("git --work-tree=/etc clean -fd"), [path("/etc", CriticalWhy::SystemDir)]);
    assert_eq!(
        targets_in("git clean -fd ~/p/lib", Some(HOME)),
        [path(LIB, CriticalWhy::ProjectRoot)]
    );
    assert_eq!(targets("GIT_WORK_TREE=/x git clean -fd"), [unreadable("GIT_WORK_TREE=/x")]);
    // git's options with a separate value, from the one table (DL28).
    let home = [path(HOME, CriticalWhy::Home)];
    assert_eq!(targets("git --config-env a.b=HOME -C ~ clean -fdx"), home);
    assert_eq!(targets("git --attr-source HEAD -C ~ clean -fdx"), home);
    assert_eq!(targets("git --shallow-file f -C ~ clean -fdx"), home);
    assert_eq!(targets("false && git --config-env a.b=HOME -C ~ clean -fdx"), home);
    assert_eq!(targets("git --work-tree /etc clean -fd"), [path("/etc", CriticalWhy::SystemDir)]);
    assert_eq!(targets_in("git --attr-source HEAD clean -fdx", Some(HOME)), home);
}

#[test]
fn not_critical_git_clean_in_the_project_root() {
    assert_eq!(targets("git clean -fdx"), []);
    assert_eq!(targets("git clean -fd ."), []);
    assert_eq!(targets("git clean -fd build"), []);
    assert_eq!(targets_in("git -C ~/p/app clean -fdx", Some(HOME)), []);
}

#[test]
fn not_critical_git_clean_without_d() {
    assert_eq!(targets_in("git clean -f", Some(HOME)), []);
    assert_eq!(targets_in("git clean -fdn", Some(HOME)), []);
    assert_eq!(targets_in("git clean --dry-run -fd", Some(HOME)), []);
    assert_eq!(targets_in("git status", Some(HOME)), []);
}

#[rstest]
#[case::sudo("sudo rm -rf /", path("/", CriticalWhy::Root))]
#[case::sudo_user("sudo -u root rm -rf /", path("/", CriticalWhy::Root))]
#[case::nice("nice rm -rf ~", path(HOME, CriticalWhy::Home))]
#[case::nice_level("nice -n 19 ionice -c 3 rm -rf ~", path(HOME, CriticalWhy::Home))]
#[case::setsid("setsid rm -rf ~", path(HOME, CriticalWhy::Home))]
#[case::nohup("nohup rm -rf ~ &", path(HOME, CriticalWhy::Home))]
#[case::timeout("timeout 5 rm -rf ~", path(HOME, CriticalWhy::Home))]
#[case::env("env X=1 rm -rf ~", path(HOME, CriticalWhy::Home))]
#[case::path("/usr/bin/rm -rf ~", path(HOME, CriticalWhy::Home))]
#[case::escaped("\\rm -rf ~", path(HOME, CriticalWhy::Home))]
#[case::xargs_unknown_producer("ls | xargs rm -rf", unreadable("xargs rm -rf"))]
#[case::xargs_no_producer("xargs -0 rm -rf < list", unreadable("xargs -0 rm -rf"))]
#[case::substitution("echo $(rm -rf ~)", path(HOME, CriticalWhy::Home))]
fn critical_behind_wrappers(#[case] line: &str, #[case] expected: CriticalTarget) {
    assert_eq!(targets(line), [expected], "{line:?}");
}

#[test]
fn not_critical_find_xargs_with_literal_root() {
    assert_eq!(targets("find build -name '*.o' -print0 | xargs -0 rm -rf"), []);
    assert_eq!(targets("find ~/Downloads -mtime +30 | xargs rm -rf"), []);
    // A critical or unreadable root keeps the input unreadable.
    assert_eq!(targets("find ~ -name '*.o' | xargs rm -rf"), [unreadable("xargs rm -rf")]);
    assert_eq!(targets("find \"$X\" | xargs rm -rf"), [unreadable("xargs rm -rf")]);
}

#[rstest]
#[case::bash("bash -c 'rm -rf ~'", path(HOME, CriticalWhy::Home))]
#[case::sh_double_quotes("sh -c \"rm -rf $HOME\"", path(HOME, CriticalWhy::Home))]
#[case::zsh_cluster("zsh -lc 'cd / && rm -rf usr'", path("/usr", CriticalWhy::SystemDir))]
#[case::eval("eval 'rm -rf ~'", path(HOME, CriticalWhy::Home))]
#[case::eval_words("eval rm -rf /", path("/", CriticalWhy::Root))]
#[case::sudo_sh("sudo sh -c 'rm -rf /etc'", path("/etc", CriticalWhy::SystemDir))]
fn critical_bash_c_literal_is_scanned(#[case] line: &str, #[case] expected: CriticalTarget) {
    assert_eq!(targets(line), [expected], "{line:?}");
}

#[test]
fn a_script_file_is_not_read() {
    assert_eq!(targets("bash ./clean.sh"), []);
    assert_eq!(targets("python3 -c 'import shutil; shutil.rmtree(\"/home/u\")'"), []);
}

#[rstest]
#[case::and("false && rm -rf ~", path(HOME, CriticalWhy::Home))]
#[case::if_false("if false; then rm -rf /; fi", path("/", CriticalWhy::Root))]
#[case::or("true || rm -rf ~", path(HOME, CriticalWhy::Home))]
#[case::comment_after("false && rm -rf ~ # never", path(HOME, CriticalWhy::Home))]
fn critical_dead_branch_is_read(#[case] line: &str, #[case] expected: CriticalTarget) {
    assert_eq!(targets(line), [expected], "{line:?}");
}

#[test]
fn critical_cd_literal_target_resolves() {
    assert_eq!(targets("cd build && rm -rf out"), []);
    assert_eq!(targets("cd build; cd sub && rm -rf out"), []);
    assert_eq!(targets("cd / && rm -rf usr"), [path("/usr", CriticalWhy::SystemDir)]);
    assert_eq!(targets("cd && rm -rf .config"), [path("/home/u/.config", CriticalWhy::HoldsEfr)]);
    assert_eq!(
        targets("cd $HOME && rm -rf .ssh"),
        [path("/home/u/.ssh", CriticalWhy::HoldsSecret)]
    );
    // A `cd` can fail: the dir before it counts too.
    assert_eq!(
        targets_in("cd /tmp/x; rm -rf p", Some(HOME)),
        [path("/home/u/p", CriticalWhy::HoldsProjects)]
    );
    // A `cd` that efr cannot read leaves the dir unknown.
    assert_eq!(targets("cd \"$X\" && rm -rf out"), [unreadable("out")]);
    assert_eq!(targets("cd - && rm -rf out"), [unreadable("out")]);
    assert_eq!(targets("pushd a; popd; rm -rf out"), [unreadable("out")]);
    assert_eq!(targets("cd \"$X\" && rm -rf /etc"), [path("/etc", CriticalWhy::SystemDir)]);
}

#[test]
fn critical_brace_words_are_expanded() {
    assert_eq!(
        targets("rm -rf ~/{p,Downloads/x}"),
        [path("/home/u/p", CriticalWhy::HoldsProjects)]
    );
    assert_eq!(targets("rm -rf build/{a,b}"), []);
    assert_eq!(targets("rm -rf ~/{1..3}"), [path(HOME, CriticalWhy::Home)]);
    // Many groups read as a pattern, at once.
    let many = format!("rm -rf ~/{}", "{a,b}".repeat(40));
    assert_eq!(targets(&many), [path(HOME, CriticalWhy::Home)]);
    assert_eq!(targets(&format!("rm -rf build/{}", "{a,b}".repeat(40))), []);
}

proptest! {
    #[test]
    fn critical_deletes_reads_any_line(line in "[a-z~/$*{},.\"'`;&|()<>=\\\\ \\n-]{0,80}") {
        found_in(&line, Some(APP));
        found_in(&line, None);
    }

    #[test]
    fn a_delete_of_home_after_plain_commands_is_always_found(
        before in "[a-z0-9 ./-]{0,40}",
        joint in prop::sample::select(vec!["; ", " && ", " || ", " | ", "\n", " & "]),
    ) {
        let line = format!("{before}{joint}rm -rf ~");
        prop_assert!(targets(&line).contains(&path(HOME, CriticalWhy::Home)), "{line:?}");
    }
}

#[test]
fn each_target_of_a_simple_command_is_found_once_in_order() {
    let found = found_in("rm -rf / ~ /; sudo rm -rf ~", Some(APP));
    let found: Vec<(&str, &CriticalTarget)> =
        found.iter().map(|delete| (delete.part.as_str(), &delete.target)).collect();
    assert_eq!(
        found,
        [
            ("rm -rf / ~ /", &path("/", CriticalWhy::Root)),
            ("rm -rf / ~ /", &path(HOME, CriticalWhy::Home)),
            ("sudo rm -rf ~", &path(HOME, CriticalWhy::Home)),
        ]
    );
}

/// The machine of the `auto` engine tests (`engine/tests/auto.rs`): `~`, `/`, `/home`,
/// `/srv/site` and `~/p/app` registered, `~/p/lib` an envelope root.
fn auto_machine() -> Locations {
    let mut locations = Locations::new(HOME)
        .unwrap()
        .with_sealed_root(format!("{DATA}/secrets"))
        .unwrap()
        .with_project(id(1), APP)
        .unwrap()
        .with_project(id(2), HOME)
        .unwrap()
        .with_project(id(3), "/")
        .unwrap()
        .with_project(id(4), "/home")
        .unwrap()
        .with_project(id(5), "/srv/site")
        .unwrap()
        .with_write_sealed_root("/home/u/.config/efr")
        .unwrap()
        .with_synced_root("/home/u/Dropbox")
        .unwrap();
    for root in ["/tmp", "/var/tmp", "/dev/shm"] {
        locations = locations.with_private_root(root).unwrap();
    }
    for root in ["/home/u/.cargo", "/home/u/.cache"] {
        locations = locations.with_cache_root(root).unwrap();
    }
    locations.with_envelope_root(LIB).unwrap().with_trash_root(format!("{DATA}/trash")).unwrap()
}

#[test]
fn above_root_approver_is_unchanged() {
    use Approver::{ClassifierWhenNamed as Named, User};
    // The `above_root` paths of the engine tests, with the approver they assert.
    let rows: &[(&str, Approver)] = &[
        // exits_above_root_of_one_project_is_classifier_when_named
        ("/home/u/p/lib", Named),
        (APP, Named),
        // exits_above_root_home_and_two_projects_are_user, exits_rm_dotdot_is_above_root
        ("/home/u/p", User),
        (HOME, User),
        ("/", User),
        ("/home", User),
        ("/home/u/.config", User),
        ("/home/u/.local/share", User),
        // Outside home, and a dir that holds nothing.
        ("/srv/site", User),
        ("/home/u/Documents", Named),
        ("/home/u/Dropbox", Named),
    ];
    let locations = auto_machine();
    let envelope = Envelope::new(&locations, Some(Path::new(APP)), Some(Path::new(SCRATCH)));
    for (path, approver) in rows {
        assert_eq!(envelope.above_root_approver(Path::new(path)), *approver, "{path}");
    }
    // exits_above_root_that_holds_a_floor_or_a_secret_is_user, write_to_efr_data_root
    let locations = auto_machine()
        .with_envelope_root("/home/u/p/two")
        .unwrap()
        .with_envelope_root("/home/u/p/keys")
        .unwrap()
        .with_floor_root("/home/u/p/two/bin")
        .unwrap()
        .with_secret_root("/home/u/p/keys/id")
        .unwrap()
        .with_efr_root("/home/u/.local/state/efr", [])
        .unwrap();
    let envelope = Envelope::new(&locations, Some(Path::new(APP)), Some(Path::new(SCRATCH)));
    for path in ["/home/u/p/two", "/home/u/p/keys", "/home/u/.local/state"] {
        assert_eq!(envelope.above_root_approver(Path::new(path)), User, "{path}");
    }
    // The approver is the user exactly when `holds` says why, or for the places that
    // it does not count: at or above home, and outside it.
    for (path, approver) in rows {
        let path = Path::new(path);
        let place = locations.is_at_or_above_home(path) || !path.starts_with(HOME);
        let user = place || envelope.holds(path).is_some();
        assert_eq!(user, *approver == User, "{}", path.display());
    }
}
