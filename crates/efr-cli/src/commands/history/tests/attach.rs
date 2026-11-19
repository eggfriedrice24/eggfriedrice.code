//! The attachments of a transcript (TM-Attach 6 and 9): the chip row under the model row
//! of its prompt, 2 columns in, the row of those that efrd stops to send where it
//! happened, and the detail of a `read_file` call of an image.

use std::path::PathBuf;

use efr_config::DiffGroups;
use efr_protocol::{
    AttachmentId, AttachmentOrigin, CommandId, EffectiveSettings, Event, Mode, Origin,
    OverriddenSettings, Scope, TurnSettings, WithheldReason,
};
use efr_render::RenderOptions;
use jiff::tz::TimeZone;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::{Shown, transcript};
use super::page_of;
use crate::testing::{call, conversation, shot, turn};

fn id(n: u32) -> AttachmentId {
    AttachmentId::nth(n)
}

fn events() -> Vec<Event> {
    let settings = EffectiveSettings {
        mode: Mode::Cautious,
        model: "claude-opus-5-5".to_owned(),
        effort: Some("high".to_owned()),
        provider: None,
        many_accounts: false,
        overridden: OverriddenSettings::default(),
        fallback: None,
        default_effort: None,
    };
    let mut read = shot();
    read.id = id(2);
    read.origin = AttachmentOrigin::Tool { path: PathBuf::from("/home/you/p/app/docs/shot.png") };
    read.copy = None;
    vec![
        Event::PromptQueued {
            turn_id: turn(),
            command_id: "019a9b1c-3d00-7a10-8b20-0000000000c1".parse::<CommandId>().unwrap(),
            text: "why is the header cut off in @docs/shot.png".to_owned(),
            origin: Origin::Shell,
            context: None,
            settings: TurnSettings::default(),
            steers: Vec::new(),
            attachments: vec![shot()],
        },
        Event::TurnStarted {
            turn_id: turn(),
            cwd: "/home/you/p/app".into(),
            scope: Scope::Machine,
            settings: Some(settings),
            unread_attachments: Vec::new(),
        },
        Event::AttachmentsWithheld {
            turn_id: Some(turn()),
            attachments: vec![id(1)],
            reason: WithheldReason::Refused,
        },
        Event::ToolCallStarted {
            turn_id: turn(),
            call_id: call(),
            tool: "read_file".to_owned(),
            input: json!({ "path": "/home/you/p/app/docs/shot.png" }),
            manual_input: false,
            launch: None,
            freeform: false,
        },
        Event::ToolCallCompleted {
            turn_id: turn(),
            call_id: call(),
            output: "shot.png: PNG image, 1920x1080, 2.1 MB".to_owned(),
            truncated: false,
            is_error: false,
            exit_code: None,
            sandbox: None,
            refusal: None,
            changes: None,
            diff: None,
            left_running: false,
            attachments: vec![read],
        },
    ]
}

fn shown(file_links: bool) -> String {
    let options = RenderOptions::new(80).with_terminal(false);
    let shown = Shown {
        options: &options,
        verbose: false,
        home: None,
        groups: &DiffGroups::default(),
        zone: &TimeZone::UTC,
        head: None,
        file_links,
    };
    transcript(conversation(), &page_of(events()), &shown)
}

#[test]
fn the_chip_row_stands_under_the_model_row_and_a_refusal_where_it_happened() {
    let text = shown(true);
    let rows: Vec<&str> = text.lines().skip_while(|row| !row.starts_with('\u{203a}')).collect();
    assert_eq!(
        rows[..5],
        [
            "\u{203a} why is the header cut off in @docs/shot.png  12:00",
            "  claude-opus-5-5 high \u{b7} cautious",
            "  @ image shot.png 1920x1080 2.1 MB",
            "  not sent: shot.png. The provider refused it; the model gets the path.",
            "  \u{2713} read docs/shot.png  image 1920x1080",
        ],
        "{text}"
    );
}

#[test]
fn the_chip_row_links_only_where_a_link_can_open_the_file() {
    let painted = |file_links: bool| {
        let options = RenderOptions::new(80);
        let shown = Shown {
            options: &options,
            verbose: false,
            home: None,
            groups: &DiffGroups::default(),
            zone: &TimeZone::UTC,
            head: None,
            file_links,
        };
        transcript(conversation(), &page_of(events()), &shown)
    };
    let link = "\x1b]8;;file:///home/you/.local/share/efr/scratch/s/attachments/a1-shot.png\x1b\\";
    assert!(painted(true).contains(link));
    assert!(!painted(false).contains("\x1b]8;;file://"));
    assert!(!shown(true).contains('\x1b'), "a pipe gets plain text");
}
