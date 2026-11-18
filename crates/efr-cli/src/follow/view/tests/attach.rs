//! The attachments of a followed turn: the chip row of its prompt at its start, after
//! the `⇄` line of the same batch (TM-Attach 2), the row of the attachments that efrd
//! stops to send (TM-Attach 9), and the detail of a `read_file` call of an image
//! (TM-Attach 5).

use std::path::PathBuf;

use efr_protocol::{
    Attachment, AttachmentId, AttachmentKind, AttachmentOrigin, CommandId, Event, Origin, Scope,
    SentForm, SwitchedModel, TurnSettings, WithheldReason,
};
use efr_render::RenderOptions;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::TurnView;
use crate::terminal::Size;
use crate::testing::{Grid, bare, now, turn};

const SIZE: Size = Size { cols: 80, rows: 20 };

fn id(n: u32) -> AttachmentId {
    format!("a{n}").parse().unwrap()
}

fn shot() -> Attachment {
    Attachment {
        id: id(1),
        name: "shot.png".to_owned(),
        media_type: "image/png".to_owned(),
        bytes: 2_100_000,
        sha256: "00".repeat(32),
        kind: AttachmentKind::Image { width: 1920, height: 1080 },
        origin: AttachmentOrigin::Path { path: PathBuf::from("/home/you/p/app/docs/shot.png") },
        copy: Some(PathBuf::from("/home/you/.local/share/efr/scratch/s/attachments/a1-shot.png")),
        sent: Some(SentForm {
            media_type: "image/jpeg".to_owned(),
            bytes: 400_000,
            width: Some(1920),
            height: Some(1080),
        }),
    }
}

fn queued(attachments: Vec<Attachment>) -> Event {
    Event::PromptQueued {
        turn_id: turn(),
        command_id: "019a9b1c-3d00-7a10-8b20-0000000000c1".parse::<CommandId>().unwrap(),
        text: "why is the header cut off in @docs/shot.png".to_owned(),
        origin: Origin::Shell,
        context: None,
        settings: TurnSettings::default(),
        steers: Vec::new(),
        attachments,
    }
}

fn started(unread: Vec<AttachmentId>) -> Event {
    Event::TurnStarted {
        turn_id: turn(),
        cwd: PathBuf::from("/home/you/p/app"),
        scope: Scope::Machine,
        settings: None,
        unread_attachments: unread,
    }
}

fn switched() -> Event {
    let side = |model: &str| SwitchedModel {
        provider: Some("anthropic-api".to_owned()),
        model: model.to_owned(),
        effort: Some("high".to_owned()),
        default_effort: None,
    };
    Event::ModelSwitched {
        turn_id: turn(),
        from: side("claude-sonnet-5"),
        to: side("claude-opus-5-5"),
    }
}

fn reply(text: &str) -> Event {
    Event::AssistantMessageCompleted { turn_id: turn(), index: 0, text: text.to_owned() }
}

/// What the screen and stderr show after `events`, a frame after each, without colour.
fn shown(view: &mut TurnView, events: &[Event]) -> (String, String) {
    let mut grid = Grid::new(SIZE.cols);
    let mut err = String::new();
    for event in events {
        let step = view.event(event, SIZE, false);
        grid.write(&step.out);
        err.push_str(&step.err);
        grid.write(&view.frame(SIZE, now()));
    }
    (bare(&grid.lines().join("\n")), err)
}

fn terminal() -> TurnView {
    TurnView::new(turn(), RenderOptions::new(80)).with_file_links(true)
}

#[test]
fn the_chip_row_stands_after_the_line_of_a_switch_and_before_the_reply() {
    let mut view = terminal();
    let (out, _) = shown(
        &mut view,
        &[queued(vec![shot()]), started(Vec::new()), switched(), reply("The height is fixed.")],
    );
    assert_eq!(
        out,
        "\u{21c4} claude-opus-5-5 high  was claude-sonnet-5 high\n\
         @ image shot.png 1920x1080 2.1 MB\n\
         \n\
         The height is fixed."
    );
}

#[test]
fn the_chip_row_shows_from_the_start_of_the_turn_and_says_what_goes_as_a_path() {
    let mut view = terminal();
    let (out, _) = shown(&mut view, &[queued(vec![shot()]), started(vec![id(1)])]);
    assert!(out.contains("@ image shot.png 1920x1080 2.1 MB as a path"), "{out}");
}

#[test]
fn the_name_links_to_the_copy_unless_efr_runs_over_ssh() {
    let link = "\x1b]8;;file:///home/you/.local/share/efr/scratch/s/attachments/a1-shot.png\x1b\\";
    for (links, linked) in [(true, true), (false, false)] {
        let mut view = TurnView::new(turn(), RenderOptions::new(80)).with_file_links(links);
        let mut out = view.event(&queued(vec![shot()]), SIZE, false).out;
        out.push_str(&view.event(&started(Vec::new()), SIZE, false).out);
        out.push_str(&view.event(&reply("Fixed."), SIZE, false).out);
        out.push_str(&view.frame(SIZE, now()));
        assert_eq!(out.contains(link), linked, "{out:?}");
    }
}

#[test]
fn without_a_terminal_the_chip_row_goes_to_stderr_at_the_start() {
    let mut view = TurnView::new(turn(), RenderOptions::new(80).with_terminal(false));
    let (_, err) = shown(&mut view, &[queued(vec![shot()]), started(Vec::new())]);
    assert_eq!(err, "@ image shot.png 1920x1080 2.1 MB\n");
}

#[test]
fn a_prompt_with_no_attachment_has_no_chip_row() {
    let mut view = terminal();
    let (out, err) = shown(&mut view, &[queued(Vec::new()), started(Vec::new()), reply("Hi.")]);
    assert!(!out.contains('@') && !err.contains('@'), "{out}{err}");
}

#[test]
fn the_row_of_a_refusal_names_the_file_where_it_happens() {
    let mut view = terminal();
    let withheld = Event::AttachmentsWithheld {
        turn_id: Some(turn()),
        attachments: vec![id(1)],
        reason: WithheldReason::Refused,
    };
    let (out, _) = shown(&mut view, &[queued(vec![shot()]), started(Vec::new()), withheld]);
    assert!(
        out.contains("not sent: shot.png. The provider refused it; the model gets the path."),
        "{out}"
    );
}

/// The start and the end of call `n` of `read_file` on `path`, which read an image of
/// the size of [`shot`] when `image`.
fn read(n: u8, path: &str, image: bool) -> [Event; 2] {
    let call_id = format!("019a9b1c-3d00-7a10-8b20-0000000001{n:02}").parse().unwrap();
    let started = Event::ToolCallStarted {
        turn_id: turn(),
        call_id,
        tool: "read_file".to_owned(),
        input: json!({ "path": path }),
        manual_input: false,
        launch: None,
        freeform: false,
    };
    let mut attachment = shot();
    attachment.origin = AttachmentOrigin::Tool { path: PathBuf::from(path) };
    attachment.copy = None;
    let completed = Event::ToolCallCompleted {
        turn_id: turn(),
        call_id,
        output: String::new(),
        truncated: false,
        is_error: false,
        exit_code: None,
        sandbox: None,
        refusal: None,
        changes: None,
        diff: None,
        left_running: false,
        attachments: if image { vec![attachment] } else { Vec::new() },
    };
    [started, completed]
}

#[test]
fn a_read_of_an_image_names_its_pixels_after_its_path() {
    let mut view = TurnView::new(turn(), RenderOptions::new(80).with_terminal(false));
    let [start, end] = read(1, "docs/shot.png", true);
    let (_, err) = shown(&mut view, &[started(Vec::new()), start, end]);
    assert_eq!(err, "\u{2713} read docs/shot.png  image 1920x1080\n");
}

#[test]
fn a_folded_run_of_reads_names_its_images_at_its_end() {
    let mut events = vec![started(Vec::new())];
    events.extend(read(1, "src/ui/header.tsx", false));
    events.extend(read(2, "docs/shot.png", true));
    events.push(reply("Two lines."));
    let (out, _) = shown(&mut terminal(), &events);
    assert!(
        out.starts_with(
            "\u{2713} read 2 files  src/ui/header.tsx, docs/shot.png  image 1920x1080\n"
        ),
        "{out}"
    );
    let mut events = vec![started(Vec::new())];
    events.extend(read(1, "docs/a.png", true));
    events.extend(read(2, "docs/b.png", true));
    events.push(reply("Two images."));
    let (out, _) = shown(&mut terminal(), &events);
    assert!(out.starts_with("\u{2713} read 2 files  docs/a.png, b.png  2 images\n"), "{out}");
}
