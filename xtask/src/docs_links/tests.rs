use super::{
    Docs, Link, Markdown, Page, Renderer, SUMMARY, check, heading_ids, links, load, resolve, slug,
};

/// A tree of pages, with every file and directory that they make.
fn docs(pages: &[(&str, &str)]) -> Docs {
    let mut docs = Docs::default();
    for (path, text) in pages {
        docs.files.insert((*path).to_owned());
        let mut dir = *path;
        while let Some((parent, _)) = dir.rsplit_once('/') {
            docs.dirs.insert(parent.to_owned());
            dir = parent;
        }
        if path.ends_with(".md") {
            docs.pages.push(Page {
                path: (*path).to_owned(),
                text: (*text).to_owned(),
                outside: false,
            });
        }
    }
    docs
}

fn messages(docs: &Docs) -> Vec<String> {
    check(docs).problems.iter().map(ToString::to_string).collect()
}

const SUMMARY_TEXT: &str =
    "# Summary\n\n[efr](README.md)\n\n- [Install](start/install.md)\n- [Sandbox](sandbox.md)\n";

fn book(install: &str) -> Docs {
    docs(&[
        (SUMMARY, SUMMARY_TEXT),
        ("README.md", "# efr\n"),
        ("start/install.md", install),
        (
            "sandbox.md",
            "# The auto sandbox\n\n## Exits\n\n## Exits\n\n## The `efr sandbox` command\n",
        ),
        ("config.schema.json", "{}"),
    ])
}

#[test]
fn slug_follows_mdbook() {
    // The cases of mdBook 0.5.4's own test of id_from_content.
    assert_eq!(slug("`--passes`: add more rustdoc passes"), "--passes-add-more-rustdoc-passes");
    assert_eq!(slug("Method-call \u{1f419} expressions \u{1f47c}"), "method-call--expressions-");
    assert_eq!(slug("_-_12345"), "_-_12345");
    assert_eq!(slug("\u{dc}ber"), "\u{fc}ber");
    assert_eq!(slug("Foo & Bar"), "foo--bar");
    assert_eq!(slug("  Trimmed  "), "trimmed");
}

#[test]
fn heading_ids_number_repeated_headings() {
    let ids = heading_ids("# A\n\n## A\n\n### A\n", Renderer::MdBook(Markdown::default()));
    assert_eq!(ids.len(), 3);
    for id in ["a", "a-1", "a-2"] {
        assert!(ids.contains(id), "{id}");
    }
}

#[test]
fn heading_ids_take_code_spans_and_leave_out_image_text() {
    let ids = heading_ids(
        "## The `efr sandbox` command ![logo](x.png)\n",
        Renderer::MdBook(Markdown::default()),
    );
    assert!(ids.contains("the-efr-sandbox-command"), "{ids:?}");
}

#[test]
fn smart_punctuation_changes_the_id_on_the_site_only() {
    let text = "## The --dir flag\n";
    assert!(heading_ids(text, Renderer::MdBook(Markdown::default())).contains("the-dir-flag"));
    assert!(heading_ids(text, Renderer::GitHub).contains("the---dir-flag"));
}

#[test]
fn without_smart_punctuation_the_ids_agree() {
    let markdown = Markdown { smart_punctuation: false, definition_lists: true };
    let ids = heading_ids("## The --dir flag\n", Renderer::MdBook(markdown));
    assert!(ids.contains("the---dir-flag"), "{ids:?}");
    let mut docs = book("# Install\n\n## The --dir flag\n\n[x](#the---dir-flag)\n");
    docs.markdown = markdown;
    assert_eq!(messages(&docs), Vec::<String>::new());
}

#[test]
fn book_toml_sets_the_options_of_output_html_only() {
    let text = "[book]\nsmart-punctuation = true\n\n[output.html]\n# A comment.\n\
        smart-punctuation = false # keep --\ndefinition-lists=false\n\n[output.html.print]\n\
        enable = false\n";
    assert_eq!(
        Markdown::from_book_toml(text),
        Markdown { smart_punctuation: false, definition_lists: false }
    );
    assert_eq!(Markdown::from_book_toml("[book]\ntitle = \"efr\"\n"), Markdown::default());
}

#[test]
fn an_explicit_id_works_on_the_site_only() {
    let text = "## Exits {#leave}\n\n## Leave\n";
    let site = heading_ids(text, Renderer::MdBook(Markdown::default()));
    assert!(site.contains("leave"), "{site:?}");
    // The explicit id does not count, so the next heading takes `leave` too: one id.
    assert_eq!(site.len(), 1);
    let github = heading_ids(text, Renderer::GitHub);
    assert!(github.contains("exits-leave"), "{github:?}");
}

#[test]
fn definition_terms_get_ids_on_the_site() {
    let ids = heading_ids("Term\n: The meaning.\n", Renderer::MdBook(Markdown::default()));
    assert!(ids.contains("term"), "{ids:?}");
}

#[test]
fn links_have_their_lines_and_reference_links_resolve() {
    let text = "# T\n\nSee [a](a.md) and\n[b][b] and <https://x.org> and <u@x.org>.\n\n![i](i.png)\n\n[b]: b.md#c\n";
    let found = links(text);
    let expected = [("a.md", 3), ("b.md#c", 4), ("https://x.org", 4), ("i.png", 6)];
    let expected: Vec<Link> = expected
        .iter()
        .map(|(dest, line)| Link { dest: (*dest).to_owned(), line: *line })
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn links_in_code_are_not_links() {
    assert!(links("`[a](b.md)`\n\n```text\n[a](b.md)\n```\n").is_empty());
}

#[test]
fn resolve_stays_inside_docs() {
    assert_eq!(resolve("start", "../sandbox.md").as_deref(), Some("sandbox.md"));
    assert_eq!(resolve("", "./adr/./x.md").as_deref(), Some("adr/x.md"));
    assert_eq!(resolve("start", "..").as_deref(), Some(""));
    assert_eq!(resolve("", "../README.md"), None);
}

#[test]
fn a_clean_book_passes() {
    let install = "# Install\n\nSee [exits](../sandbox.md#exits), [again](../sandbox.md#exits-1), \
        [cmd](../sandbox.md#the-efr-sandbox-command), [top](#install), [home](../), \
        [schema](../config.schema.json), [web](https://example.org/x.md) and \
        [mail](mailto:u@example.org).\n";
    let docs = book(install);
    let report = check(&docs);
    assert_eq!(report.problems, []);
    assert_eq!(report.links, 3 + 8);
}

#[test]
fn a_missing_file_fails() {
    assert_eq!(
        messages(&book("# Install\n\n[x](../nope.md)\n")),
        ["docs/start/install.md:3: `../nope.md`: docs/nope.md does not exist"]
    );
}

#[test]
fn a_missing_anchor_fails_and_names_the_side() {
    assert_eq!(
        messages(&book("# Install\n\n[x](../sandbox.md#exit)\n\n[y](#nope)\n")),
        [
            "docs/start/install.md:3: `../sandbox.md#exit`: no heading of docs/sandbox.md makes the id `exit` on the site or on GitHub",
            "docs/start/install.md:5: `#nope`: no heading of docs/start/install.md makes the id `nope` on the site or on GitHub",
        ]
    );
    let docs = book("# Install\n\n## The --dir flag\n\n[x](#the---dir-flag)\n");
    assert_eq!(
        messages(&docs),
        [
            "docs/start/install.md:5: `#the---dir-flag`: no heading of docs/start/install.md makes the id `the---dir-flag` on the site"
        ]
    );
}

#[test]
fn a_link_out_of_docs_or_from_the_root_fails() {
    assert_eq!(
        messages(&book("# Install\n\n[a](../../ARCHITECTURE.md)\n\n[b](/sandbox.md)\n")),
        [
            "docs/start/install.md:3: `../../ARCHITECTURE.md` leaves docs/, which is all the site holds; use a full URL",
            "docs/start/install.md:5: `/sandbox.md` starts with /, which leaves the site's path; use a relative link",
        ]
    );
}

#[test]
fn a_link_to_a_readme_fails_and_its_directory_passes() {
    assert_eq!(
        messages(&book("# Install\n\n[a](../README.md)\n")),
        [
            "docs/start/install.md:3: `../README.md` names a README.md, which mdBook writes as index.html; link to its directory (`../`)"
        ]
    );
    assert_eq!(messages(&book("# Install\n\n[a](../#efr)\n")), Vec::<String>::new());
    assert_eq!(
        messages(&book("# Install\n\n[a](../start/)\n")),
        [
            "docs/start/install.md:3: `../start/` is a directory with no README.md page; link to a page"
        ]
    );
}

#[test]
fn a_page_outside_summary_and_a_listed_page_that_is_missing_fail() {
    let mut docs = book("# Install\n");
    docs.pages.push(Page {
        path: "extra.md".to_owned(),
        text: "# Extra\n".to_owned(),
        outside: false,
    });
    docs.pages[0].text.push_str("- [Gone](gone.md)\n- [Draft]()\n");
    assert_eq!(
        messages(&docs),
        [
            "docs/SUMMARY.md:7: lists `gone.md`, which is not a .md file in docs/",
            "docs/extra.md: is not in SUMMARY.md; list it, or move it out of docs/",
        ]
    );
}

#[test]
fn a_book_without_summary_fails() {
    assert_eq!(
        messages(&docs(&[("README.md", "# efr\n")])),
        ["docs/SUMMARY.md: does not exist; the book needs it"]
    );
}

#[test]
fn a_page_from_outside_docs_takes_no_relative_file_link() {
    let mut docs = book("# Install\n");
    docs.pages[0].text.push_str("- [Architecture](internals/architecture.md)\n");
    docs.pages.push(Page {
        path: "internals/architecture.md".to_owned(),
        text: "# Architecture\n\n[a](#architecture) [b](crates/x.rs) [c](https://x.org)\n"
            .to_owned(),
        outside: true,
    });
    assert_eq!(
        messages(&docs),
        [
            "docs/internals/architecture.md:3: `crates/x.rs` is a relative link in a page that is a symlink to a file outside docs/; it resolves from another directory on GitHub; use a full URL"
        ]
    );
}

#[test]
fn load_follows_symlinks_and_marks_the_pages_from_outside() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::create_dir_all(root.join("docs/internals")).unwrap();
    std::fs::write(root.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
    std::fs::write(
        root.join("docs/SUMMARY.md"),
        "# Summary\n\n- [A](internals/architecture.md)\n- [B](b.md)\n",
    )
    .unwrap();
    std::fs::write(root.join("docs/b.md"), "# B\n\n[top](#b)\n").unwrap();
    std::fs::write(root.join("docs/config.schema.json"), "{}").unwrap();
    std::fs::write(root.join("docs/inside.md"), "# Inside\n").unwrap();
    std::os::unix::fs::symlink(
        "../../ARCHITECTURE.md",
        root.join("docs/internals/architecture.md"),
    )
    .unwrap();
    std::os::unix::fs::symlink("../b.md", root.join("docs/internals/b-link.md")).unwrap();

    let docs = load(root).unwrap();
    let pages: Vec<(&str, bool)> =
        docs.pages.iter().map(|page| (page.path.as_str(), page.outside)).collect();
    assert_eq!(
        pages,
        [
            ("SUMMARY.md", false),
            ("b.md", false),
            ("inside.md", false),
            ("internals/architecture.md", true),
            ("internals/b-link.md", false),
        ]
    );
    assert!(docs.files.contains("config.schema.json"));
    assert!(docs.dirs.contains("internals"));
    assert_eq!(
        messages(&docs),
        [
            "docs/inside.md: is not in SUMMARY.md; list it, or move it out of docs/",
            "docs/internals/b-link.md: is not in SUMMARY.md; list it, or move it out of docs/",
        ]
    );
}

#[test]
fn an_off_site_page_needs_no_summary_line_and_takes_no_link_from_the_site() {
    let mut docs = book("# Install\n\nSee [the decision](../adr/0001-x.md).\n");
    docs.files.insert("adr/0001-x.md".to_owned());
    docs.dirs.insert("adr".to_owned());
    docs.pages.push(Page {
        path: "adr/0001-x.md".to_owned(),
        text: "# 0001 X\n\nSee [the sandbox](../sandbox.md).\n".to_owned(),
        outside: false,
    });
    assert_eq!(
        messages(&docs),
        [
            "docs/start/install.md:3: `../adr/0001-x.md` is below docs/adr, which the site leaves out; \
          use a full GitHub URL"
        ]
    );
}
