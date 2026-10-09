# efr-patch

## Purpose

The patch engine. It reads the text of an `apply_patch` call and computes the new
content of each file. It does no IO: the caller reads the files, gives their text to
the engine, and writes the result. The same engine serves the `apply_patch` tool and
the `edit` tool with an old and a new string (`replace`), which Claude models get.

The format is the `apply_patch` format of Codex. `GRAMMAR` is its Lark grammar,
written for efr: it describes the same language as
`codex-rs/core/assets/tools/apply_patch.lark`, so a model that was trained on that
format writes text that it accepts. A provider sends the grammar with the freeform
`apply_patch` tool.

```text
*** Begin Patch
*** Add File: docs/new.md
+# New
*** Delete File: old.txt
*** Update File: src/lib.rs
*** Move to: src/core.rs
@@ impl Engine
@@     fn run(&self) {
-        old();
+        new();
         done();
*** End of File
*** End Patch
```

## API contract

The entry points are pure functions. Other crates build on these types and
signatures. A change to them is a change of this contract.

- `parse(text: &str) -> Result<Patch, PatchError>`. A `Patch` holds its
  `operations` in the order of the text. An `Operation` is one of:
  - `Add { path, content }`: the text of the `+` lines, each line with its newline.
  - `Delete { path }`.
  - `Update { path, move_to, hunks }`: `move_to` is the target of
    `*** Move to:`. A `Hunk` has its `anchors` (the text of each `@@ <text>` line
    before it, in order; a bare `@@` adds none), its `lines` (`HunkLine::Context`,
    `Remove` and `Add`, without the marker and the newline) and `end_of_file` (after
    `*** End of File`).

  Paths stay as the text writes them. The caller resolves them with
  `Patch::map_paths`. `Patch::paths` gives each path once, in order, with the target
  of a move after its source. `Operation::is_destructive` is true for a delete and
  a move. Text that is not a patch is `PatchError::Parse { line, problem }`, with
  the line from 1 and a `ParseProblem`.

- `apply(patch: &Patch, files: &dyn Files) -> Result<Vec<FileChange>, PatchError>`.
  `Files::text(path)` gives the current text of a path, or `None` when no file is
  there. `HashMap<PathBuf, String>` and `BTreeMap<PathBuf, String>` implement it.
  The rules:
  - The engine computes every new content before it returns. It writes nothing, so
    the caller can write all changes or none.
  - The operations apply in order. Each operation sees the result of the operations
    before it: a file that an earlier operation removed is absent, and a file that
    it added is present.
  - The result has one `FileChange { path, kind, from }` for each path that changes, in
    the order that the patch first names the path. A later operation on the same
    path folds into that change. `ChangeKind` is `Added { content }`,
    `Updated { content }`, `Deleted` or `Moved { to, content }`. A path whose text
    ends as it started has no change. `Moved` is for a file that leaves a path that
    had a file for a path that had none. Any other result of a move shows as a
    delete and an add or an update. Then `from` of the add or the update names the
    path that the file had before the patch, so the caller keeps that file's mode.
    It is `None` for a new file, a file that stays at its path, a delete and a
    `Moved`.
  - An add of a file that exists, and a move onto a file that exists, are
    `PatchError::Exists { path }`. An update, a delete or a move of a file that does
    not exist is `PatchError::Missing { path }`. A move to its own path is an update.
  - The hunks of one update apply from the top of the file down. The search for a
    hunk starts after the hunk before it.
  - Each `@@ <text>` anchor names the first line after the anchor before it (after
    the previous hunk for the first anchor) that it matches. An anchor matches a
    line in the passes below, and then in one more pass: the line starts with the
    anchor, and the anchor does not end inside a word (a letter, a digit or `_`
    after it). So `@@ fn run()` names the line `fn run() {`, but `@@ impl Foo` does
    not name `impl FooBar {` and `@@ fn run` does not name `fn run_all() {`. A
    missing anchor is
    `PatchError::NoAnchor { path, hunk, anchor, nearest }`.
  - The old lines of a hunk (its context and `-` lines) match in passes. The first
    pass is exact. Then the engine ignores trailing whitespace. Then it ignores all
    whitespace around each line. Then it also reads Unicode punctuation (dashes,
    quotes, special spaces) as ASCII. The first pass that finds a match decides.
  - With an anchor, the search starts at the line of the last anchor, and the first
    match wins. Without an anchor, the match must be the only one after the
    previous hunk. Else the hunk is `PatchError::Ambiguous { path, hunk, lines }`,
    with the first line of each match, so the model can add context or an anchor.
  - A hunk with `end_of_file` must match at the end of the file.
  - A hunk that has no old lines inserts its lines after its last anchor. Without an
    anchor, or with `end_of_file`, it appends them to the file, as in Codex. Such a
    hunk does not move the start of the search for the next hunk.
  - When no pass finds a hunk and its last old line is empty, the engine tries once
    more without that line. Models often write the final newline of a file as an
    empty context line. A hunk whose only old line is that empty line is not tried
    again, because without it the hunk would append to the file.
  - A hunk that matches nowhere is `PatchError::NoMatch { path, hunk, nearest }`.
    `hunk` counts from 1 in its update. `nearest` holds at most 12 `NearLine
    { number, text }` of the file: the place that shares the most lines with the
    hunk (else the line most like its longest line), with one line before and after.
  - Context lines keep the text and the line ending that they have in the file, so a
    tolerant match changes only the `-` and `+` lines. A new line takes the first
    line ending of the file (`\n` when the file has none). The new content keeps the
    final newline of the old file, or the lack of it. An empty file counts as
    ending in a newline.

- `replace(text, old, new, occurrences: Occurrences) -> Result<Replacement,
  PatchError>`, for the edit tool. The match is exact, with no tolerant pass.
  `Occurrences::One` needs exactly one occurrence, else
  `PatchError::NotUnique { count }`. `Occurrences::All` replaces every
  occurrence. An `old` that does not occur is `PatchError::NotFound { nearest }`,
  with the lines nearest to `old` as for `NoMatch`, and an empty `old` is
  `PatchError::EmptyOld`. `Replacement` holds the new `text` and the `count` of the
  replacements.

`PatchError` is the one error type. Its `Display` text is one sentence. `efr-tools`
writes the longer text that the model reads from the fields.

## What the parser accepts

The parser is strict on structure: each problem is a `ParseProblem` at its line.
It accepts these variants, as the parser of Codex does:

- Blank lines before `*** Begin Patch` and after `*** End Patch`.
- Whitespace around `*** Begin Patch`, `*** End Patch` and the operation lines
  (`*** Add File:` and the others). Inside an update, only trailing whitespace: a
  line that starts with a space is a context line, even when it looks like a
  marker.
- `\r\n` line ends in the patch, and no newline after `*** End Patch`.
- A patch in a heredoc: a first line `<<EOF`, `<<'EOF'` or `<<"EOF"`, and a last line
  that ends with `EOF`.
- An empty line in a hunk, read as an empty context line.
- No `@@` before the first hunk of an update, and no `*** End of File`.
- Blank lines after `*** End of File`.
- An add with no `+` line: an empty file.

It also accepts these variants, which Codex refuses:

- Several `@@` lines in a row. Each one narrows the search for the next, such as
  `@@ impl Engine` and then `@@ fn run(&self) {`.
- A unified diff header as an anchor: `@@ -12,7 +12,8 @@ fn run()` reads as
  `@@ fn run()`, and `@@ -12,7 +12,8 @@` as a bare `@@`.
- An update with a move and no hunk: a rename.
- Blank lines between operations, except in an added file, where every line must
  start with `+`.
- No space between a marker and its path, and whitespace around the path.

## Tier

Tier 0. It depends on no workspace crate.

## Allowed dependencies

No workspace crate. Third-party crates: `thiserror`. `xtask/src/deps.rs` holds the
allowlist and forbids `efr-patch -> tokio` through any chain. `efr-tools` may depend
on this crate (for `apply_patch` and the later edit tool), and so may `efr-daemon`.

## Invariant

The engine is pure. It reads no file, writes no file and has no runtime. A patch
applies completely or not at all: the engine returns every new content or an error,
and the caller writes only after a success.

## Tests

Run the tests of this crate alone, without the rest of the workspace:

```sh
cargo nextest run -p efr-patch
```

The unit tests sit next to each module (`src/<module>/tests.rs`). The integration
binary `tests/it` holds the property tests of `roundtrip.rs`: a patch made from a
diff of two random texts turns the first text into the second. With repeated lines,
the patch gives the second text or `PatchError::Ambiguous`, never a wrong text.

The tests use no file system, no network and no Zig.
