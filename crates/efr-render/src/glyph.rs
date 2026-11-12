//! [`Glyphs`]: every glyph of efr's own lines, in one table with a Unicode form and an
//! ASCII form, so a terminal whose locale is not UTF-8 gets marks it can show.
//!
//! The table holds each glyph of the design's legend (`TM-Legend`) and the mark of
//! the thought row. Each Unicode glyph is one cell wide and in every monospace font:
//! no emoji and no Nerd Font glyph. The CLI picks the table from the locale
//! (`LC_ALL`, then `LC_CTYPE`, then `LANG`, as the zsh plugin does) and passes it in
//! [`RenderOptions::with_glyphs`](crate::RenderOptions::with_glyphs); a line reads
//! its marks from [`RenderOptions::glyphs`](crate::RenderOptions::glyphs). No other
//! file of `efr-render` or `efr-cli` writes one of these glyphs as a literal (a test
//! of `efr-cli` checks it).
//!
//! Only two ASCII forms are decided by the design: home is `~` and the prompt mark is
//! `>`. The others are the proposal of the build spec, easy to change here.

/// The glyphs of efr's own lines. Each field is the glyph as text, because some ASCII
/// forms take more than one character (`=>`, `...`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Glyphs {
    /// Between the parts of a line, such as `app · auto` or `exit 2 · 1.5s`: `·`
    /// (U+00B7), ASCII `-`.
    pub separator: &'static str,
    /// The place outside every project, home: `⌂` (U+2302), ASCII `~`.
    pub home: &'static str,
    /// The prompt mark of sticky mode, a steer and the input row: `›` (U+203A), ASCII
    /// `>`.
    pub prompt: &'static str,
    /// The mark of the line that says the conversation changed: `↪` (U+21AA), ASCII
    /// `=>`.
    pub conversation: &'static str,
    /// The mark of the line that says the model, the provider or the account changed:
    /// `⇄` (U+21C4), ASCII `<>`.
    pub switch: &'static str,
    /// An arrow inside a line, such as `a → b` or `231k → 24k`: `→` (U+2192), ASCII
    /// `->`.
    pub arrow: &'static str,
    /// The mark of a call moved to a job, in the scan column: `→` (U+2192), ASCII `>`
    /// (one cell, so the column stays one cell wide).
    pub moved: &'static str,
    /// The mark of a call that went well, a done turn and an allowed question: `✓`
    /// (U+2713), ASCII `+`.
    pub ok: &'static str,
    /// The mark of a failed call, a failed turn and a denied question: `✗` (U+2717),
    /// ASCII `x`.
    pub failed: &'static str,
    /// The mark of a call that the sandbox refused: `!` in both forms.
    pub refused: &'static str,
    /// The mark of the reason why the model stopped: `!` in both forms.
    pub stopped: &'static str,
    /// The mark of a call that the user denied, so it did not run: `·` (U+00B7), ASCII
    /// `-`.
    pub denied: &'static str,
    /// The mark of an interrupted turn: `■` (U+25A0), ASCII `#`.
    pub interrupted: &'static str,
    /// The mark of the thought row: `▸` (U+25B8), ASCII `*`.
    pub thought: &'static str,
    /// The mark of an unread steer and of a queued prompt: `↳` (U+21B3), ASCII `|`.
    pub waiting: &'static str,
    /// The end of a row of a command that is cut at a space: `\` in both forms.
    pub wrap: &'static str,
    /// The end of a row that is cut inside a word, such as a row of a diff: `↩`
    /// (U+21A9), ASCII `<`.
    pub word_cut: &'static str,
    /// What stands for text that does not show, such as `… 14 more lines`: `…`
    /// (U+2026), ASCII `...`.
    pub more: &'static str,
    /// One cell of a rule, such as the line under the header of a table: `─` (U+2500),
    /// ASCII `-`.
    pub rule: &'static str,
    /// The frames of the spinner, in turn: the braille dots `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` (U+280B to
    /// U+280F), ASCII `-`, `\`, `|`, `/`.
    pub spinner: &'static [&'static str],
    /// The spinner when `render.motion` is off: `•` (U+2022), ASCII `*`.
    pub still: &'static str,
    /// The badge of a question: `?` in both forms.
    pub question: &'static str,
    /// The mark before a shell command: `$` in both forms.
    pub shell: &'static str,
    /// The start of a hunk header of a diff: `@@` in both forms.
    pub hunk: &'static str,
}

impl Glyphs {
    /// The glyphs for a UTF-8 locale.
    pub const UNICODE: Glyphs = Glyphs {
        separator: "\u{b7}",
        home: "\u{2302}",
        prompt: "\u{203a}",
        conversation: "\u{21aa}",
        switch: "\u{21c4}",
        arrow: "\u{2192}",
        moved: "\u{2192}",
        ok: "\u{2713}",
        failed: "\u{2717}",
        refused: "!",
        stopped: "!",
        denied: "\u{b7}",
        interrupted: "\u{25a0}",
        thought: "\u{25b8}",
        waiting: "\u{21b3}",
        wrap: "\\",
        word_cut: "\u{21a9}",
        more: "\u{2026}",
        rule: "\u{2500}",
        spinner: &[
            "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
            "\u{2827}", "\u{2807}", "\u{280f}",
        ],
        still: "\u{2022}",
        question: "?",
        shell: "$",
        hunk: "@@",
    };

    /// The glyphs for a locale that is not UTF-8: ASCII only.
    pub const ASCII: Glyphs = Glyphs {
        separator: "-",
        home: "~",
        prompt: ">",
        conversation: "=>",
        switch: "<>",
        arrow: "->",
        moved: ">",
        ok: "+",
        failed: "x",
        refused: "!",
        stopped: "!",
        denied: "-",
        interrupted: "#",
        thought: "*",
        waiting: "|",
        wrap: "\\",
        word_cut: "<",
        more: "...",
        rule: "-",
        spinner: &["-", "\\", "|", "/"],
        still: "*",
        question: "?",
        shell: "$",
        hunk: "@@",
    };

    /// The table for a locale: [`Glyphs::UNICODE`] when `utf8`, else
    /// [`Glyphs::ASCII`].
    pub const fn for_locale(utf8: bool) -> &'static Glyphs {
        if utf8 { &Glyphs::UNICODE } else { &Glyphs::ASCII }
    }

    /// Every glyph of the table with its name, in the order of the fields; the spinner
    /// gives one entry for each frame, named `spinner`. For a test that checks a glyph
    /// against the design or looks for a glyph in source files.
    pub fn all(&self) -> Vec<(&'static str, &'static str)> {
        let mut all = vec![
            ("separator", self.separator),
            ("home", self.home),
            ("prompt", self.prompt),
            ("conversation", self.conversation),
            ("switch", self.switch),
            ("arrow", self.arrow),
            ("moved", self.moved),
            ("ok", self.ok),
            ("failed", self.failed),
            ("refused", self.refused),
            ("stopped", self.stopped),
            ("denied", self.denied),
            ("interrupted", self.interrupted),
            ("thought", self.thought),
            ("waiting", self.waiting),
            ("wrap", self.wrap),
            ("word_cut", self.word_cut),
            ("more", self.more),
            ("rule", self.rule),
        ];
        all.extend(self.spinner.iter().map(|frame| ("spinner", *frame)));
        all.extend([
            ("still", self.still),
            ("question", self.question),
            ("shell", self.shell),
            ("hunk", self.hunk),
        ]);
        all
    }
}

impl Default for Glyphs {
    fn default() -> Self {
        Glyphs::UNICODE
    }
}

#[cfg(test)]
mod tests;
