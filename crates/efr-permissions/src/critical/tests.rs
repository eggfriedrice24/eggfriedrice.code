use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::{CriticalDelete, CriticalTarget, CriticalWhy};

const HOME: &str = "/home/u";

fn delete(path: &str, why: CriticalWhy) -> CriticalDelete {
    CriticalDelete {
        part: format!("rm -rf {path}"),
        target: CriticalTarget::Path { path: PathBuf::from(path), why },
    }
}

#[test]
fn a_critical_delete_names_its_target_below_home_with_a_tilde() {
    let home = Path::new(HOME);
    assert_eq!(delete(HOME, CriticalWhy::Home).describe(home), "~ (your home directory)");
    assert_eq!(delete("/", CriticalWhy::Root).describe(home), "/ (the root directory)");
    assert_eq!(
        delete("/home/u/p/app", CriticalWhy::ProjectRoot).describe(home),
        "~/p/app (a project)"
    );
    assert_eq!(delete("/etc", CriticalWhy::SystemDir).describe(home), "/etc (a system directory)");
    assert_eq!(
        delete("/home/user2", CriticalWhy::SystemDir).describe(home),
        "/home/user2 (a system directory)",
        "a path that only starts with the same letters is not below home"
    );
}

#[test]
fn an_unreadable_target_is_the_word_as_the_line_wrote_it() {
    let delete = CriticalDelete {
        part: "rm -rf \"$X\"/*".to_owned(),
        target: CriticalTarget::Unreadable { word: "\"$X\"/*".to_owned() },
    };
    assert_eq!(delete.describe(Path::new(HOME)), "\"$X\"/* (efr cannot read the target)");
    assert_eq!(delete.target.to_string(), "\"$X\"/* (efr cannot read the target)");
}
