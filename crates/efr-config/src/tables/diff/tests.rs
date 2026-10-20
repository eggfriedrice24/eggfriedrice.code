use pretty_assertions::assert_eq;

use super::DiffGroups;

fn groups(list: &[(&str, &[&str])]) -> DiffGroups {
    DiffGroups::new(
        list.iter()
            .map(|(name, globs)| {
                ((*name).to_owned(), globs.iter().map(|glob| (*glob).to_owned()).collect())
            })
            .collect(),
    )
}

#[test]
fn the_first_group_that_matches_names_the_file() {
    let groups = groups(&[("evals", &["evals/", "*.eval.toml"]), ("golden", &["evals/golden/"])]);

    assert_eq!(groups.group_of("evals/a.toml"), Some("evals"));
    assert_eq!(groups.group_of("evals/golden/a.txt"), Some("evals"));
    assert_eq!(groups.group_of("src/x.eval.toml"), Some("evals"));
    assert_eq!(groups.group_of("src/main.rs"), None);
    assert_eq!(DiffGroups::default().group_of("evals/a.toml"), None);
}

#[test]
fn the_groups_write_as_a_table_in_their_order() {
    let groups = groups(&[("zeta", &["z/"]), ("alpha", &["a/"])]);

    let text = toml::to_string(&groups).unwrap();

    assert_eq!(text, "zeta = [\"z/\"]\nalpha = [\"a/\"]\n");
}

#[test]
fn order_as_takes_the_order_of_the_file_and_keeps_the_rest_last() {
    let mut groups = groups(&[("alpha", &["a/"]), ("other", &["o/"]), ("zeta", &["z/"])]);

    groups.order_as(&["zeta".to_owned(), "alpha".to_owned()]);

    let names: Vec<&str> = groups.iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["zeta", "alpha", "other"]);
}
