use efr_protocol::{Attachment, AttachmentId, AttachmentKind, AttachmentOrigin, SentForm};
use pretty_assertions::assert_eq;

use super::{
    AttachmentPart, EncodedBytes, IMAGE_MAX_TOKENS, LABEL_CLOSE, MAX_DATA_BYTES, MAX_IMAGE_BLOCKS,
    attachment_tokens, fit, label,
};
use crate::{ContentBlock, Message, Role};

const SCRATCH: &str = "/home/u/.local/share/efr/scratch/2026-10-20-disk/attachments";

fn image(n: u32, width: u32, height: u32) -> Attachment {
    Attachment {
        id: AttachmentId::nth(n),
        name: format!("shot-{n}.png"),
        media_type: "image/png".to_owned(),
        bytes: 2_100_000,
        sha256: "ab".repeat(32),
        kind: AttachmentKind::Image { width, height },
        origin: AttachmentOrigin::Path { path: format!("/home/u/Pictures/shot-{n}.png").into() },
        copy: Some(format!("{SCRATCH}/a{n}-shot-{n}.png").into()),
        sent: Some(SentForm {
            media_type: "image/png".to_owned(),
            bytes: 400_000,
            width: Some(width),
            height: Some(height),
        }),
    }
}

fn pdf(n: u32, pages: Option<u32>, bytes: u64) -> Attachment {
    Attachment {
        id: AttachmentId::nth(n),
        name: "report.pdf".to_owned(),
        media_type: "application/pdf".to_owned(),
        bytes,
        sha256: "cd".repeat(32),
        kind: AttachmentKind::Pdf { pages },
        origin: AttachmentOrigin::Phone,
        copy: Some(format!("{SCRATCH}/a{n}-report.pdf").into()),
        sent: Some(SentForm {
            media_type: "application/pdf".to_owned(),
            bytes,
            width: None,
            height: None,
        }),
    }
}

/// `attachment` with data of `bytes` base64 bytes.
fn with_data(attachment: Attachment, bytes: usize) -> AttachmentPart {
    AttachmentPart { attachment, data: Some(EncodedBytes::new("A".repeat(bytes))) }
}

fn prompt(parts: Vec<AttachmentPart>) -> Message {
    let mut content: Vec<ContentBlock> = parts.into_iter().map(ContentBlock::Attachment).collect();
    content.push(ContentBlock::Text { text: "what is this".to_owned() });
    Message::new(Role::User, content)
}

fn results(parts: Vec<AttachmentPart>) -> Message {
    Message::new(
        Role::User,
        vec![ContentBlock::ToolResult {
            call_id: "call_1".to_owned(),
            output: "shot.png: PNG image".to_owned(),
            is_error: false,
            attachments: parts,
        }],
    )
}

fn ids(range: std::ops::RangeInclusive<u32>) -> Vec<AttachmentId> {
    range.map(AttachmentId::nth).collect()
}

#[test]
fn an_image_of_a_path_names_its_copy_and_the_users_file() {
    let shown = label(&image(1, 1920, 1080), true);
    assert_eq!(
        shown.open,
        format!(
            "<attachment id=\"a1\" name=\"shot-1.png\" type=\"image/png\" \
             size=\"1920x1080, 2.1 MB\" path=\"{SCRATCH}/a1-shot-1.png\" \
             from=\"/home/u/Pictures/shot-1.png\">"
        )
    );
    assert_eq!(shown.close, LABEL_CLOSE);
    let not_sent = label(&image(1, 1920, 1080), false);
    assert_eq!(
        not_sent.open,
        format!("{}\n[efr: not sent as content. Open the file at the path.]", shown.open)
    );
}

#[test]
fn a_text_file_of_a_path_points_at_the_users_file_and_names_the_copy() {
    let text = Attachment {
        id: AttachmentId::nth(3),
        name: "main.rs".to_owned(),
        media_type: "text/plain".to_owned(),
        bytes: 14_200,
        kind: AttachmentKind::Text,
        origin: AttachmentOrigin::Path { path: "/home/u/p/src/main.rs".into() },
        copy: Some(format!("{SCRATCH}/a3-main.rs").into()),
        sent: None,
        ..image(3, 1, 1)
    };
    assert_eq!(
        label(&text, false).open,
        format!(
            "<attachment id=\"a3\" name=\"main.rs\" type=\"text/plain\" size=\"14 KB\" \
             path=\"/home/u/p/src/main.rs\" copy=\"{SCRATCH}/a3-main.rs\">\n\
             [efr: not sent as content. The user's file is at the path. The copy is the file \
             as it was when the prompt was sent.]"
        )
    );
}

#[test]
fn each_origin_has_its_path_and_a_pdf_its_pages() {
    let tool = Attachment {
        origin: AttachmentOrigin::Tool { path: "/home/u/p/plot.png".into() },
        copy: None,
        ..image(4, 800, 600)
    };
    assert!(label(&tool, true).open.ends_with("path=\"/home/u/p/plot.png\">"));
    let phone = label(&pdf(2, Some(12), 310_000), true).open;
    assert!(
        phone.ends_with(&format!("size=\"12 pages, 310 KB\" path=\"{SCRATCH}/a2-report.pdf\">")),
        "{phone}"
    );
    let one = label(&pdf(2, Some(1), 900), true).open;
    assert!(one.contains("size=\"1 page, 900 B\""), "{one}");
    let clipboard = Attachment {
        origin: AttachmentOrigin::Clipboard,
        kind: AttachmentKind::Other,
        ..pdf(5, None, 7)
    };
    let open = label(&clipboard, false).open;
    assert!(open.contains(&format!("size=\"7 B\" path=\"{SCRATCH}/a5-report.pdf\">")), "{open}");
    assert!(!open.contains("from=") && !open.contains("copy="), "{open}");
}

#[test]
fn a_name_cannot_close_the_tag_or_break_the_line() {
    let evil = Attachment {
        name: "a\"><b>&c\nd.png".to_owned(),
        origin: AttachmentOrigin::Phone,
        ..image(1, 2, 2)
    };
    let open = label(&evil, true).open;
    assert!(open.contains("name=\"a___b__c d.png\""), "{open}");
    assert_eq!(open.matches('>').count(), 1, "{open}");
    assert_eq!(open.matches('<').count(), 1, "{open}");
}

#[test]
fn only_parts_with_data_count_tokens_and_an_image_has_a_top() {
    let small = with_data(image(1, 1000, 500), 10);
    let huge = with_data(image(2, 2576, 2576), 10);
    let none = AttachmentPart::new(image(3, 1000, 500));
    // NOTE: ceil(1000 / 28) * ceil(500 / 28) = 36 * 18 = 648.
    assert_eq!(attachment_tokens(&prompt(vec![small.clone()])), 648);
    assert_eq!(attachment_tokens(&prompt(vec![huge])), IMAGE_MAX_TOKENS);
    assert_eq!(attachment_tokens(&prompt(vec![none])), 0);
    assert_eq!(attachment_tokens(&results(vec![small])), 648, "a tool result counts too");
}

#[test]
fn a_three_megabyte_image_counts_under_five_thousand_tokens() {
    let photo = Attachment { bytes: 3_000_000, ..image(1, 2000, 1500) };
    let tokens = attachment_tokens(&prompt(vec![with_data(photo, 4_000_000)]));
    assert!(tokens < 5_000, "{tokens}");
}

#[test]
fn a_pdf_counts_its_pages_or_its_bytes() {
    assert_eq!(attachment_tokens(&prompt(vec![with_data(pdf(1, Some(3), 9_000), 10)])), 13_500);
    assert_eq!(attachment_tokens(&prompt(vec![with_data(pdf(1, None, 40_000), 10)])), 10_000);
}

#[test]
fn within_each_limit_nothing_is_withheld() {
    let parts = (1..=90).map(|n| with_data(image(n, 28, 28), 10)).collect();
    assert_eq!(fit(&[prompt(parts)], 200_000), Vec::new());
    assert_eq!(fit(&[], 200_000), Vec::new());
}

#[test]
fn ninety_one_images_keep_forty_five_with_data() {
    let parts = (1..=91).map(|n| with_data(image(n, 28, 28), 10)).collect();
    let withheld = fit(&[results(parts)], 200_000);
    assert_eq!(withheld, ids(1..=46));
    assert_eq!(91 - withheld.len(), MAX_IMAGE_BLOCKS / 2);
}

#[test]
fn more_than_a_hundred_megabytes_fall_to_half() {
    let each = 10_100_000;
    let parts = (1..=10).map(|n| with_data(image(n, 28, 28), each)).collect();
    let withheld = fit(&[results(parts)], 200_000);
    let left = (10 - withheld.len()) as u64 * each as u64;
    assert!(left <= MAX_DATA_BYTES / 2, "{left}");
    assert_eq!(withheld, ids(1..=6));
}

#[test]
fn a_pdf_over_half_of_the_window_is_withheld() {
    let withheld = fit(&[prompt(vec![with_data(pdf(1, Some(30), 1_000), 1_000)])], 200_000);
    assert_eq!(withheld, ids(1..=1));
}

#[test]
fn a_part_over_the_limit_alone_goes_and_the_older_parts_stay() {
    let older = results((1..=5).map(|n| with_data(image(n, 2000, 1500), 10)).collect());
    let newest = prompt(vec![with_data(pdf(6, Some(30), 1_000), 1_000)]);
    assert_eq!(fit(&[older, newest], 200_000), ids(6..=6));
}

#[test]
fn the_parts_of_the_newest_prompt_go_last() {
    let old = prompt(vec![with_data(image(1, 28, 28), 10)]);
    let calls = results((2..=60).map(|n| with_data(image(n, 28, 28), 10)).collect());
    let newest = prompt((61..=91).map(|n| with_data(image(n, 28, 28), 10)).collect());
    let withheld = fit(&[old, calls, newest], 200_000);
    assert_eq!(withheld, ids(1..=46), "the newest prompt keeps its images");
}

#[test]
fn a_part_with_no_data_never_counts() {
    let parts = (1..=200).map(|n| AttachmentPart::new(image(n, 28, 28))).collect();
    assert_eq!(fit(&[results(parts)], 200_000), Vec::new());
}
