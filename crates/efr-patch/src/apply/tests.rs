use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;

use super::Files;

#[test]
fn a_map_gives_the_text_of_a_path_and_none_for_a_missing_one() {
    let mut hashed: HashMap<PathBuf, String> = HashMap::new();
    hashed.insert(PathBuf::from("/w/a.txt"), "a\n".to_owned());
    let sorted: BTreeMap<PathBuf, String> = hashed.clone().into_iter().collect();
    for files in [&hashed as &dyn Files, &sorted as &dyn Files] {
        assert_eq!(files.text(Path::new("/w/a.txt")), Some("a\n"));
        assert_eq!(files.text(Path::new("/w/b.txt")), None);
    }
}
