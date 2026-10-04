//! One sample document per element the renderer draws. The streaming property tests
//! in `renderer/tests.rs` build documents out of them.

pub(crate) const ELEMENTS: &[(&str, &str)] = &[
    (
        "headings",
        "# Heading one\n\n## Heading two\n\n### Heading three\n\nSetext heading\n==============\n",
    ),
    (
        "emphasis",
        "Some *italic*, **bold**, ***both***, ~~struck~~ and `inline code` in one line of prose.\n",
    ),
    (
        "paragraphs",
        "A first paragraph on one line.\nIts second line stays a line of its own.\n\nA second paragraph long enough to pass forty columns, which the terminal wraps, not the renderer.\n",
    ),
    (
        "long_paragraph",
        "line one of a long paragraph\nline two\nline three\nline four\nline five\nline six\n",
    ),
    (
        "bullet_list",
        "- one\n- two, with text long enough to wrap inside the item at forty columns\n  - nested\n    - deeper\n- three\n",
    ),
    (
        "ordered_list",
        "1. first\n1. second\n1. third\n\n   with a second paragraph in the third item\n1. fourth\n",
    ),
    (
        "task_list",
        "- [ ] to do\n- [x] done\n- [ ] a task whose text is long enough to wrap at forty columns\n",
    ),
    (
        "quote",
        "> A quote long enough to wrap when the terminal is forty columns wide.\n>\n> Its second paragraph.\n>\n> - a list inside the quote\n",
    ),
    (
        "code_rust",
        "```rust\nfn main() {\n    let greeting = \"hello\"; // a comment\n    println!(\"{greeting}, from a line that is wider than forty columns\");\n}\n```\n",
    ),
    ("code_plain", "```\nplain text\n\tafter a tab\n```\n"),
    ("code_in_list", "1. Build it:\n\n   ```sh\n   cargo build --release\n   ```\n2. Done.\n"),
    (
        "diff",
        "```diff\ndiff --git a/src/main.rs b/src/main.rs\nindex 1111111..2222222 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@ fn main() {\n fn main() {\n-    println!(\"old\");\n+    println!(\"new\");\n }\n```\n",
    ),
    (
        "table",
        "| Name | Kind | Size |\n|:-----|:----:|-----:|\n| `efr-render` | lib | 1 |\n| efrd | **bin** | 22 |\n",
    ),
    (
        "table_wide",
        "| Option | Meaning |\n|---|---|\n| width | the number of columns text is laid out in |\n| theme | the syntax theme of code blocks and diffs |\n",
    ),
    (
        "links",
        "See [the docs](https://example.com/docs), <https://example.com>, https://example.com/bare, [hosts](/etc/hosts), `/var/log/syslog` and [a relative link](src/lib.rs).\n",
    ),
    ("image", "![a diagram](https://example.com/diagram.png)\n"),
    ("rule", "above\n\n---\n\nbelow\n"),
    ("html", "<details>\n<summary>More</summary>\n</details>\n"),
];

/// The sample for `name`.
pub(crate) fn element(name: &str) -> &'static str {
    ELEMENTS.iter().find(|(element, _)| *element == name).map_or("", |(_, markdown)| markdown)
}
