use std::path::PathBuf;

use efr_protocol::{
    Attachment, AttachmentId, AttachmentKind, AttachmentOrigin, SentForm, WithheldReason,
};
use efr_render::RenderOptions;
use pretty_assertions::assert_eq;

use super::{Known, chip_rows};
use crate::format::Tone;
use crate::testing::bare;

fn id(n: u32) -> AttachmentId {
    format!("a{n}").parse().unwrap()
}

/// An attachment `a<n>` named `name` of `bytes` and `kind`, with its copy, sent as
/// content when it is an image or a PDF.
fn attachment(n: u32, name: &str, bytes: u64, kind: AttachmentKind) -> Attachment {
    let sent = matches!(kind, AttachmentKind::Image { .. } | AttachmentKind::Pdf { .. })
        .then(|| SentForm { media_type: "image/png".to_owned(), bytes, width: None, height: None });
    Attachment {
        name: name.to_owned(),
        bytes,
        kind,
        origin: AttachmentOrigin::Path { path: PathBuf::from(format!("/home/you/p/app/{name}")) },
        copy: Some(PathBuf::from(format!(
            "/home/you/.local/share/efr/scratch/s/attachments/a{n}-{name}"
        ))),
        sent,
        ..efr_test_support::attachments::image(n, 1, 1)
    }
}

fn image(n: u32, name: &str, bytes: u64, width: u32, height: u32) -> Attachment {
    attachment(n, name, bytes, AttachmentKind::Image { width, height })
}

/// The ten files of TM-Attach 2c.
fn ten() -> Vec<Attachment> {
    let text = |n, name, bytes| attachment(n, name, bytes, AttachmentKind::Text);
    vec![
        image(1, "home.png", 1_100_000, 1170, 2532),
        image(2, "login.png", 980_000, 1170, 2532),
        image(3, "settings.png", 1_200_000, 1170, 2532),
        attachment(4, "report.pdf", 3_400_000, AttachmentKind::Pdf { pages: Some(12) }),
        text(5, "data.csv", 48_000),
        text(6, "schema.sql", 12_000),
        text(7, "error.log", 210_000),
        text(8, "theme.json", 4_000),
        image(9, "logo.png", 88_000, 512, 512),
        text(10, "notes.md", 6_000),
    ]
}

fn terminal(width: u16) -> RenderOptions {
    RenderOptions::new(width).with_terminal(true)
}

#[test]
fn a_chip_names_the_kind_the_name_and_the_size() {
    let three = [
        image(1, "shot.png", 2_100_000, 1920, 1080),
        attachment(2, "report.pdf", 3_400_000, AttachmentKind::Pdf { pages: None }),
        attachment(3, "data.csv", 48_000, AttachmentKind::Text),
        attachment(4, "core.bin", 512, AttachmentKind::Other),
    ];
    let row = chip_rows(&three, &[], true, &terminal(200));
    assert_eq!(
        bare(&row),
        "@ image shot.png 1920x1080 2.1 MB, pdf report.pdf 3.4 MB, text data.csv 48 KB, file core.bin 512 B\n"
    );
}

/// TM-Narrow: the chips are one row at any width, which the terminal wraps.
#[test]
fn a_row_of_chips_is_one_row_at_any_width() {
    let row = "@ image home.png 1170x2532 1.1 MB, image login.png 1170x2532 980 KB, \
         image settings.png 1170x2532 1.2 MB, pdf report.pdf 3.4 MB, text data.csv 48 KB, \
         text schema.sql 12 KB, text error.log 210 KB, text theme.json 4.0 KB, \
         image logo.png 512x512 88 KB, text notes.md 6.0 KB\n";
    assert_eq!(bare(&chip_rows(&ten(), &[], true, &terminal(100))), row);
    assert_eq!(bare(&chip_rows(&ten(), &[], true, &terminal(60))), row);
}

#[test]
fn a_file_that_the_model_gets_as_a_path_says_so() {
    let mut big = attachment(3, "manual.pdf", 18_000_000, AttachmentKind::Pdf { pages: None });
    big.sent = None;
    let shot = image(1, "shot.png", 2_100_000, 1920, 1080);
    let row = chip_rows(&[big, shot], &[id(1)], true, &terminal(200));
    assert_eq!(
        bare(&row),
        "@ pdf manual.pdf 18 MB as a path, image shot.png 1920x1080 2.1 MB as a path\n"
    );
}

#[test]
fn the_name_links_to_the_copy_only_where_a_link_can_open_it() {
    let shot = [image(1, "shot:12.png", 2_100_000, 1920, 1080)];
    let row = chip_rows(&shot, &[], true, &terminal(80).with_colour(efr_render::ColourMode::None));
    let url = "file:///home/you/.local/share/efr/scratch/s/attachments/a1-shot%3A12.png";
    assert!(row.contains(&format!("\x1b]8;;{url}\x1b\\")), "{row:?}");
    assert!(row.contains("\x1b]8;;\x1b\\"), "{row:?}");
    for (links, options) in [
        (false, terminal(80)),
        (true, terminal(80).with_hyperlinks(false)),
        (true, RenderOptions::new(80).with_terminal(false)),
    ] {
        let row = chip_rows(&shot, &[], links, &options);
        assert!(!row.contains("\x1b]8;"), "{row:?}");
        assert_eq!(bare(&row), "@ image shot:12.png 1920x1080 2.1 MB\n");
    }
}

#[test]
fn the_row_of_a_refusal_names_the_files_and_the_row_of_the_budget_counts_them() {
    let mut known = Known::default();
    known.add(&[
        image(1, "shot.png", 1, 1, 1),
        attachment(2, "report.pdf", 1, AttachmentKind::Pdf { pages: None }),
    ]);
    assert_eq!(
        known.withheld(&[id(1)], WithheldReason::Refused),
        [(
            "not sent: shot.png. The provider refused it; the model gets the path.".to_owned(),
            Tone::Attention
        )]
    );
    assert_eq!(
        known.withheld(&[id(1), id(7)], WithheldReason::Refused),
        [(
            "not sent: shot.png, a7. The provider refused them; the model gets the paths."
                .to_owned(),
            Tone::Attention
        )]
    );
    let older: Vec<AttachmentId> = (10..56).map(id).collect();
    assert_eq!(
        known.withheld(&older, WithheldReason::Budget),
        [(
            "not sent from now on: 46 older images. The request had too many; the model gets the paths."
                .to_owned(),
            Tone::Dim
        )]
    );
    assert_eq!(
        known.withheld(&[id(2)], WithheldReason::Budget),
        [("not sent from now on: 1 older file. The request had too many; the model gets the path.".to_owned(), Tone::Dim)]
    );
}

/// TM-Attach 8c: without colour the mark and `as a path` are bold, the name is
/// underlined while it is a link, and the rest is dim.
#[test]
fn without_colour_the_mark_and_the_path_words_are_bold_and_a_link_is_underlined() {
    let mut big = attachment(1, "manual.pdf", 18_000_000, AttachmentKind::Pdf { pages: None });
    big.sent = None;
    let options = terminal(80).with_colour(efr_render::ColourMode::None);
    let row = chip_rows(&[big], &[], true, &options);
    let url = "file:///home/you/.local/share/efr/scratch/s/attachments/a1-manual.pdf";
    assert_eq!(
        row,
        format!(
            "\x1b[1m@\x1b[0m \x1b[2mpdf\x1b[0m \x1b]8;;{url}\x1b\\\x1b[4mmanual.pdf\x1b[0m\x1b]8;;\x1b\\ \
             \x1b[2m18 MB\x1b[0m \x1b[1mas a path\x1b[0m\n"
        )
    );
}
