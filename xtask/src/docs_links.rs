//! `cargo xtask docs-links`: the link check of the docs site. mdBook builds the book in
//! `book/` from the pages in `docs/`, and GitHub shows the same files, so a link must
//! work in both places. The check reads `docs/SUMMARY.md` and every `.md` file below
//! `docs/` (it follows symlinks) and fails on:
//!
//! - a page that SUMMARY lists and that does not exist, or a `.md` file that SUMMARY
//!   does not list, so no page is forgotten (the pages below [`OFF_SITE`] are left out
//!   of the site on purpose: GitHub shows them, the book does not);
//! - a link from a page of the site to a page below [`OFF_SITE`], which the site does
//!   not hold: such a link must be a full GitHub URL;
//! - a relative link to a file that does not exist, a link out of `docs/` (the site
//!   holds only `docs/`) and a link that starts with `/`;
//! - a link to a `README.md` page, which mdBook writes as `index.html`;
//! - an `#anchor` that no heading of the target makes, on the site or on GitHub;
//! - a relative link to a file in a page that is a symlink to a file outside `docs/`
//!   (`ARCHITECTURE.md`, `CONVENTIONS.md`): it resolves from another directory on
//!   GitHub than on the site.
//!
//! It never fetches an external URL, so a network fault cannot fail CI. Links in raw
//! HTML are not checked.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::path::Path;

use anyhow::Context as _;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use walkdir::WalkDir;

use crate::output;

/// The source directory of the book, relative to the repository root.
pub(crate) const DOCS: &str = "docs";

/// The sidebar of the book, relative to `DOCS`.
pub(crate) const SUMMARY: &str = "SUMMARY.md";

/// The directories below `docs/` whose pages stay on GitHub only, not on the site: the
/// decision records, until the user puts them on the site.
pub(crate) const OFF_SITE: &[&str] = &["adr/"];

/// True for a path below `docs/` that the site leaves out.
fn off_site(path: &str) -> bool {
    OFF_SITE.iter().any(|dir| path.starts_with(dir))
}

/// The config of the book, relative to the repository root.
pub(crate) const BOOK_TOML: &str = "book/book.toml";

/// One Markdown file below `docs/`.
#[derive(Debug)]
pub(crate) struct Page {
    /// The path relative to `docs/`, with `/` between the parts.
    pub(crate) path: String,
    pub(crate) text: String,
    /// The file is a symlink to a file outside `docs/`.
    pub(crate) outside: bool,
}

/// What the check reads of `docs/`.
#[derive(Debug, Default)]
pub(crate) struct Docs {
    /// Every `.md` file, `SUMMARY.md` too.
    pub(crate) pages: Vec<Page>,
    /// Every file, Markdown or not, relative to `docs/`.
    pub(crate) files: BTreeSet<String>,
    /// Every directory below `docs/`, relative to it.
    pub(crate) dirs: BTreeSet<String>,
    /// How mdBook parses the pages.
    pub(crate) markdown: Markdown,
}

/// The options of `[output.html]` in `book/book.toml` that change the id of a heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Markdown {
    pub(crate) smart_punctuation: bool,
    pub(crate) definition_lists: bool,
}

/// The defaults of mdBook 0.5.4.
impl Default for Markdown {
    fn default() -> Self {
        Self { smart_punctuation: true, definition_lists: true }
    }
}

impl Markdown {
    /// The options from the text of `book.toml`. A line scan is enough: the keys take
    /// `true` or `false`, one key per line.
    pub(crate) fn from_book_toml(text: &str) -> Self {
        let mut markdown = Self::default();
        let mut section = "";
        for raw in text.lines() {
            let line = raw.trim();
            if let Some(header) = line.strip_prefix('[') {
                section = header.split(']').next().unwrap_or_default().trim();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.split('#').next().unwrap_or_default().trim();
            let Ok(value) = value.parse::<bool>() else {
                continue;
            };
            match (section, key.trim()) {
                ("output.html", "smart-punctuation") => markdown.smart_punctuation = value,
                ("output.html", "definition-lists") => markdown.definition_lists = value,
                _ => {}
            }
        }
        markdown
    }
}

/// One broken rule at one place.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    /// The page, relative to `docs/`.
    pub(crate) path: String,
    pub(crate) line: Option<usize>,
    pub(crate) message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{DOCS}/{}:{line}: {}", self.path, self.message),
            None => write!(f, "{DOCS}/{}: {}", self.path, self.message),
        }
    }
}

/// The result of one check.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) problems: Vec<Problem>,
    pub(crate) links: usize,
}

/// One link of a page: its destination and its line.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) dest: String,
    pub(crate) line: usize,
}

/// Where the ids of a page come from: mdBook or GitHub. The two parse the Markdown with
/// other options, so one heading can make two ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Renderer {
    MdBook(Markdown),
    GitHub,
}

impl Renderer {
    fn name(self) -> &'static str {
        match self {
            Self::MdBook(_) => "the site",
            Self::GitHub => "GitHub",
        }
    }

    /// The options of `mdbook_markdown::new_cmark_parser` in mdBook 0.5.4 (admonitions
    /// are always on in the book), or the GFM extensions that GitHub shows.
    fn options(self) -> Options {
        let mut options = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_GFM;
        if let Self::MdBook(markdown) = self {
            options |= Options::ENABLE_HEADING_ATTRIBUTES;
            if markdown.smart_punctuation {
                options |= Options::ENABLE_SMART_PUNCTUATION;
            }
            if markdown.definition_lists {
                options |= Options::ENABLE_DEFINITION_LIST;
            }
        }
        options
    }
}

/// The id of a heading with the text `text`: `id_from_content` of mdBook 0.5.4
/// (`mdbook-html/src/utils.rs`). GitHub makes the same id from the same text, except
/// that it turns only a space into `-`; heading text holds no other whitespace.
pub(crate) fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|ch| {
            if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                Some(ch)
            } else if ch.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

/// `id` if it is free, else the first free `id-1`, `id-2` and so on; marks it used.
fn unique(id: String, used: &mut HashSet<String>) -> String {
    if used.insert(id.clone()) {
        return id;
    }
    let mut counter = 1_u32;
    loop {
        let candidate = format!("{id}-{counter}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        counter += 1;
    }
}

/// The ids that the headings of `text` make. mdBook also gives each term of a
/// definition list an id and takes an explicit `{#id}` as it is; an explicit id does
/// not count for the numbers of the later ids, as in mdBook.
pub(crate) fn heading_ids(text: &str, renderer: Renderer) -> HashSet<String> {
    let mut ids = HashSet::new();
    let mut used = HashSet::new();
    // The text of the heading that is open, and its explicit id.
    let mut open: Option<(String, Option<String>)> = None;
    // Image alt text is not text of the heading in the HTML.
    let mut in_image = 0_usize;
    for event in Parser::new_ext(text, renderer.options()) {
        match event {
            Event::Start(Tag::Heading { id, .. }) => {
                open = Some((String::new(), id.map(|id| id.to_string())));
            }
            Event::Start(Tag::DefinitionListTitle) => open = Some((String::new(), None)),
            Event::End(TagEnd::Heading(_) | TagEnd::DefinitionListTitle) => {
                if let Some((content, explicit)) = open.take() {
                    let id = match explicit {
                        Some(id) => id,
                        None => unique(slug(&content), &mut used),
                    };
                    ids.insert(id);
                }
            }
            Event::Start(Tag::Image { .. }) => in_image += 1,
            Event::End(TagEnd::Image) => in_image = in_image.saturating_sub(1),
            Event::Text(part) | Event::Code(part) if in_image == 0 => {
                if let Some((content, _)) = open.as_mut() {
                    content.push_str(&part);
                }
            }
            // A setext heading of two lines: mdBook keeps the line break as text.
            Event::SoftBreak => {
                if let Some((content, _)) = open.as_mut() {
                    content.push('\n');
                }
            }
            _ => {}
        }
    }
    ids
}

/// The links and images of `text` as mdBook reads them, with their lines. Email
/// autolinks are left out: they are not paths.
pub(crate) fn links(text: &str) -> Vec<Link> {
    let line_starts: Vec<usize> =
        std::iter::once(0).chain(text.match_indices('\n').map(|(at, _)| at + 1)).collect();
    let line_of = |offset: usize| line_starts.partition_point(|start| *start <= offset);
    let mut found = Vec::new();
    for (event, range) in
        Parser::new_ext(text, Renderer::MdBook(Markdown::default()).options()).into_offset_iter()
    {
        let dest = match event {
            Event::Start(Tag::Link { link_type, dest_url, .. }) if link_type != LinkType::Email => {
                dest_url
            }
            Event::Start(Tag::Image { dest_url, .. }) => dest_url,
            _ => continue,
        };
        found.push(Link { dest: dest.to_string(), line: line_of(range.start) });
    }
    found
}

/// True for `https:`, `mailto:` and other links with a scheme, as mdBook tells them.
fn has_scheme(dest: &str) -> bool {
    let Some((scheme, _)) = dest.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_lowercase())
        && chars.all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '+' | '.' | '-')
        })
}

/// The directory of a page, relative to `docs/`: empty at the top.
fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// `rel` from the directory `base`, both relative to `docs/`; `None` when it leaves
/// `docs/`. The result names a directory without a trailing `/`.
pub(crate) fn resolve(base: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

fn is_readme(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.eq_ignore_ascii_case("readme.md")
}

/// The ids of each page, for both renderers, made when a link first needs them.
struct Ids<'a> {
    markdown: Markdown,
    cache: BTreeMap<(&'a str, bool), HashSet<String>>,
}

impl<'a> Ids<'a> {
    fn new(markdown: Markdown) -> Self {
        Self { markdown, cache: BTreeMap::new() }
    }

    fn renderers(&self) -> [Renderer; 2] {
        [Renderer::MdBook(self.markdown), Renderer::GitHub]
    }

    fn has(&mut self, page: &'a Page, renderer: Renderer, anchor: &str) -> bool {
        self.cache
            .entry((page.path.as_str(), renderer == Renderer::GitHub))
            .or_insert_with(|| heading_ids(&page.text, renderer))
            .contains(anchor)
    }
}

/// Runs every rule over `docs`.
pub(crate) fn check(docs: &Docs) -> Report {
    let mut report = Report::default();
    let pages: BTreeMap<&str, &Page> =
        docs.pages.iter().map(|page| (page.path.as_str(), page)).collect();
    let Some(summary) = pages.get(SUMMARY) else {
        report.problems.push(Problem {
            path: SUMMARY.to_owned(),
            line: None,
            message: "does not exist; the book needs it".to_owned(),
        });
        return report;
    };

    let listed = check_summary(summary, &pages, &mut report);
    for page in &docs.pages {
        if page.path != SUMMARY && !off_site(&page.path) && !listed.contains(page.path.as_str()) {
            report.problems.push(Problem {
                path: page.path.clone(),
                line: None,
                message: format!("is not in {SUMMARY}; list it, or move it out of {DOCS}/"),
            });
        }
    }

    let mut ids = Ids::new(docs.markdown);
    for page in &docs.pages {
        if page.path == SUMMARY {
            continue;
        }
        for link in links(&page.text) {
            report.links += 1;
            if let Some(message) = check_link(page, &link.dest, docs, &pages, &mut ids) {
                report.problems.push(Problem {
                    path: page.path.clone(),
                    line: Some(link.line),
                    message,
                });
            }
        }
    }
    report
}

/// The pages that SUMMARY lists; a listed page that does not exist is a problem.
fn check_summary<'a>(
    summary: &Page,
    pages: &BTreeMap<&'a str, &Page>,
    report: &mut Report,
) -> BTreeSet<&'a str> {
    let mut listed = BTreeSet::new();
    for link in links(&summary.text) {
        report.links += 1;
        // `[Draft]()` is a draft chapter with no page.
        if link.dest.is_empty() || has_scheme(&link.dest) {
            continue;
        }
        let path = link.dest.split('#').next().unwrap_or_default();
        let found = resolve("", path).and_then(|path| pages.get_key_value(path.as_str()));
        match found {
            Some((key, _)) => {
                listed.insert(*key);
            }
            None => report.problems.push(Problem {
                path: SUMMARY.to_owned(),
                line: Some(link.line),
                message: format!("lists `{}`, which is not a .md file in {DOCS}/", link.dest),
            }),
        }
    }
    listed
}

/// The problem of one link of `page`, if it has one.
fn check_link<'a>(
    page: &'a Page,
    dest: &str,
    docs: &Docs,
    pages: &BTreeMap<&str, &'a Page>,
    ids: &mut Ids<'a>,
) -> Option<String> {
    if dest.is_empty() || has_scheme(dest) {
        return None;
    }
    if dest.starts_with('/') {
        return Some(format!(
            "`{dest}` starts with /, which leaves the site's path; use a relative link"
        ));
    }
    let (path, anchor) = match dest.split_once('#') {
        Some((path, anchor)) => (path, Some(anchor)),
        None => (dest, None),
    };
    if path.is_empty() {
        return anchor.and_then(|anchor| missing_anchor(dest, page, anchor, ids));
    }
    if page.outside {
        return Some(format!(
            "`{dest}` is a relative link in a page that is a symlink to a file outside \
             {DOCS}/; it resolves from another directory on GitHub; use a full URL"
        ));
    }
    let Some(target) = resolve(dir_of(&page.path), path) else {
        return Some(format!(
            "`{dest}` leaves {DOCS}/, which is all the site holds; use a full URL"
        ));
    };
    if off_site(&target) && !off_site(&page.path) {
        return Some(format!(
            "`{dest}` is below {DOCS}/{}, which the site leaves out; use a full GitHub URL",
            target.split('/').next().unwrap_or_default()
        ));
    }
    let target_page = if docs.files.contains(&target) {
        if is_readme(&target) {
            let dir = dir_of(path);
            let to = if dir.is_empty() { "./".to_owned() } else { format!("{dir}/") };
            return Some(format!(
                "`{dest}` names a README.md, which mdBook writes as index.html; \
                 link to its directory (`{to}`)"
            ));
        }
        pages.get(target.as_str()).copied()
    } else if target.is_empty() || docs.dirs.contains(&target) {
        let readme =
            if target.is_empty() { "README.md".to_owned() } else { format!("{target}/README.md") };
        match pages.get(readme.as_str()) {
            Some(page) => Some(*page),
            None => {
                return Some(format!(
                    "`{dest}` is a directory with no README.md page; link to a page"
                ));
            }
        }
    } else {
        return Some(format!("`{dest}`: {DOCS}/{target} does not exist"));
    };
    match (target_page, anchor) {
        (Some(target_page), Some(anchor)) => missing_anchor(dest, target_page, anchor, ids),
        _ => None,
    }
}

fn missing_anchor<'a>(
    dest: &str,
    target: &'a Page,
    anchor: &str,
    ids: &mut Ids<'a>,
) -> Option<String> {
    let missing: Vec<&str> = ids
        .renderers()
        .into_iter()
        .filter(|renderer| !ids.has(target, *renderer, anchor))
        .map(Renderer::name)
        .collect();
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "`{dest}`: no heading of {DOCS}/{} makes the id `{anchor}` on {}",
        target.path,
        missing.join(" or on ")
    ))
}

/// Reads `docs/` below `root`, following symlinks.
pub(crate) fn load(root: &Path) -> anyhow::Result<Docs> {
    let dir = root.join(DOCS);
    let real_dir = dir.canonicalize().with_context(|| format!("reading {DOCS}/"))?;
    let mut docs = Docs::default();
    let book_toml = root.join(BOOK_TOML);
    if book_toml.is_file() {
        let text =
            std::fs::read_to_string(&book_toml).with_context(|| format!("reading {BOOK_TOML}"))?;
        docs.markdown = Markdown::from_book_toml(&text);
    }
    for entry in WalkDir::new(&dir).follow_links(true).sort_by_file_name() {
        let entry = entry.with_context(|| format!("walking {DOCS}/"))?;
        let relative = entry.path().strip_prefix(&dir).context("path outside docs/")?;
        let Some(path) = relative.to_str() else {
            anyhow::bail!("{} is not UTF-8", entry.path().display());
        };
        if path.is_empty() {
            continue;
        }
        let path = path.replace('\\', "/");
        if entry.file_type().is_dir() {
            docs.dirs.insert(path);
            continue;
        }
        docs.files.insert(path.clone());
        if !path.ends_with(".md") {
            continue;
        }
        let text = std::fs::read_to_string(entry.path())
            .with_context(|| format!("reading {DOCS}/{path}"))?;
        let outside = entry.path_is_symlink()
            && !entry
                .path()
                .canonicalize()
                .with_context(|| format!("resolving {DOCS}/{path}"))?
                .starts_with(&real_dir);
        docs.pages.push(Page { path, text, outside });
    }
    Ok(docs)
}

pub(crate) fn run(root: &Path) -> anyhow::Result<bool> {
    let docs = load(root)?;
    let report = check(&docs);
    for problem in &report.problems {
        output::line(&format!("docs-links: {problem}"));
    }
    output::line(&format!(
        "docs-links: {} pages, {} links checked, {} problems",
        docs.pages.len(),
        report.links,
        report.problems.len()
    ));
    Ok(report.problems.is_empty())
}

#[cfg(test)]
mod tests;
