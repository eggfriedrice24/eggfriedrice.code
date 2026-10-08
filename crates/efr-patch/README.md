# efr-patch

## Purpose

The patch engine. It reads the text of an `apply_patch` call and computes the new
content of each file. It does no IO: the caller reads the files, gives their text to
the engine, and writes the result. The same engine serves the `apply_patch` tool now
and an edit tool with an old and a new string later (for the Claude provider).

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
  a move. The parser is lenient where models often slip (whitespace around the
  marker lines, no final newline). Other text is `PatchError::Parse { line,
  problem }`, with the line from 1 and a `ParseProblem`.

- `apply(patch: &Patch, files: &dyn Files) -> Result<Vec<FileChange>, PatchError>`.
  `Files::text(path)` gives the current text of a path, or `None` when no file is
  there. `HashMap<PathBuf, String>` and `BTreeMap<PathBuf, String>` implement it.
  The rules:
  - The engine computes every new content before it returns. It writes nothing, so
    the caller can write all changes or none.
  - The operations apply in order. Each operation sees the result of the operations
    before it.
  - The result has one `FileChange { path, kind }` for each path that changes, in
    the order that the patch first names the path. A later operation on the same
    path folds into that change. `ChangeKind` is `Added { content }`,
    `Updated { content }`, `Deleted` or `Moved { to, content }`.
  - An add replaces a file that exists, and a move replaces its target, as in Codex.
    An update, a delete or a move of a file that does not exist is
    `PatchError::Missing { path }`.
  - A hunk matches its file in passes. The first pass is exact. Then the engine
    ignores trailing whitespace. Then it ignores all whitespace around each line.
    Then it reads Unicode punctuation (quotes, dashes, special spaces) as ASCII. The
    anchors narrow the search, in order. A hunk with `end_of_file` must match at the
    end of the file. The hunks of one update apply from the top of the file down.
  - A hunk that matches nowhere is `PatchError::NoMatch { path, hunk, nearest }`.
    `hunk` counts from 1 in its update. `nearest` holds a few `NearLine { number,
    text }` of the file that are nearest to the old lines of the hunk, so the model
    can correct the patch and try again.
  - The new content keeps the line endings and the final newline of the old file.

- `replace(text, old, new, occurrences: Occurrences) -> Result<Replacement,
  PatchError>`, for the edit tool. The match is exact. `Occurrences::One` needs
  exactly one occurrence, else `PatchError::NotUnique { count }`.
  `Occurrences::All` replaces every occurrence. An `old` that does not occur is
  `PatchError::NotFound { nearest }`, and an empty `old` is `PatchError::EmptyOld`.
  `Replacement` holds the new `text` and the `count` of the replacements.

`PatchError` is the one error type. Its `Display` text is one sentence. `efr-tools`
writes the longer text that the model reads from the fields.

The entry points `parse`, `apply` and `replace` are stubs now: they return
`PatchError::NotBuilt`. The engine work fills them in and then removes that variant.

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

The tests use no file system, no network and no Zig.
