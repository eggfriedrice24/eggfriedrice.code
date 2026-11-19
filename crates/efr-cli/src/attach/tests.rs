use std::path::{Path, PathBuf};

use efr_protocol::{
    AttachmentLimits, AttachmentOrigin, AttachmentPutResult, Capabilities, ErrorBody, ErrorCode,
    Method, MissingUploads, PromptSend, PromptSendResult, Seq, UploadId, UploadInfo,
};
use efr_stdx::text::MIB;
use efr_transport::client::{Client, ClientError};
use pretty_assertions::assert_eq;

use super::upload::checked;
use super::{Failure, Places, Token, Words, pasted_name, send, tokens};
use crate::testing::{Conn, TestEnv, conversation, hello_result, turn};

fn token(text: &str, word: &str, path: &str) -> Token {
    let start = text.find(word).unwrap();
    Token {
        range: start..start + word.len(),
        path: path.to_owned(),
        quoted: word.contains(['"', '\'']),
    }
}

#[test]
fn a_word_starts_with_an_at_sign_at_the_start_or_after_a_blank() {
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("why is @docs/shot.png cut", &[("@docs/shot.png", "docs/shot.png")]),
        ("@a.png first", &[("@a.png", "a.png")]),
        (
            "compare @\"my file.png\" with @'old logo.png'",
            &[("@\"my file.png\"", "my file.png"), ("@'old logo.png'", "old logo.png")],
        ),
        (
            r"@/home/you/new\ logo.png is new",
            &[(r"@/home/you/new\ logo.png", "/home/you/new logo.png")],
        ),
        (r"@a\(1\).png", &[(r"@a\(1\).png", "a(1).png")]),
        ("mail user@host and a@b", &[]),
        ("a lone @ and @\"open quote", &[]),
        ("tab\t@x.png\nline\n@y.png", &[("@x.png", "x.png"), ("@y.png", "y.png")]),
    ];
    for (text, expected) in cases {
        let expected: Vec<Token> =
            expected.iter().map(|(word, path)| token(text, word, path)).collect();
        assert_eq!(tokens(text), expected, "{text}");
    }
}

/// A project dir with `shot.png`, `src/main.rs`, `my file.png`, a `clips` dir with a
/// paste of shell 4242, and a home dir with `notes.md`.
struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    fn new() -> Tree {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for sub in ["project/src", "home", "clips"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(root.join("project/shot.png"), b"png!").unwrap();
        std::fs::write(root.join("project/src/main.rs"), b"fn main() {}\n").unwrap();
        std::fs::write(root.join("project/my file.png"), b"two words").unwrap();
        std::fs::write(root.join("home/notes.md"), b"# notes\n").unwrap();
        std::fs::write(root.join("clips/4242.1.png"), b"pasted").unwrap();
        Tree { dir }
    }

    fn path(&self, below: &str) -> PathBuf {
        std::fs::canonicalize(self.dir.path()).unwrap().join(below)
    }

    /// What the words of `text` name for a shell in `project`, with the tree as the
    /// state root of `efr`, so that its `clips` dir holds the pastes.
    fn find(&self, text: &str) -> Words {
        let (pwd, home, state) = (self.path("project"), self.path("home"), self.path(""));
        Words::find(text, &Places { pwd: &pwd, home: Some(&home), state: &state })
    }
}

#[test]
fn a_word_that_names_a_file_attaches_it_and_keeps_its_place() {
    let tree = Tree::new();
    let text = "compare @shot.png with @~/notes.md and @\"my file.png\" in @./src/main.rs?";
    let words = tree.find(text);
    assert_eq!(words.notes, Vec::<String>::new());
    let found: Vec<(&str, &Path, &str, u64)> = words
        .files
        .iter()
        .map(|file| {
            (&text[file.range.clone()], file.path.as_path(), file.name.as_str(), file.bytes)
        })
        .collect();
    let (shot, notes, two, main) = (
        tree.path("project/shot.png"),
        tree.path("home/notes.md"),
        tree.path("project/my file.png"),
        tree.path("project/src/main.rs"),
    );
    assert_eq!(
        found,
        [
            ("@shot.png", shot.as_path(), "shot.png", 4),
            ("@~/notes.md", notes.as_path(), "notes.md", 8),
            ("@\"my file.png\"", two.as_path(), "my file.png", 9),
            // The `?` of the sentence is no part of the path.
            ("@./src/main.rs", main.as_path(), "main.rs", 13),
        ]
    );
    assert_eq!(words.files[1].typed, "~/notes.md");
    assert!(words.files.iter().all(|file| !file.clip));
    assert_eq!(words.sent_text(text), text);
}

#[test]
fn only_a_word_that_looks_like_a_path_gets_a_note_when_it_names_nothing() {
    let tree = Tree::new();
    let quiet =
        "ask @alice why @types/node and @babel/core pin @app.route, @pytest.fixture, @v1.2.3";
    assert_eq!(tree.find(quiet), Words::default());
    let words =
        tree.find("fix @./x and @/x and @~/x and @src/mian.rs, then explain what @src does");
    assert_eq!(words.files, Vec::new());
    assert_eq!(
        words.notes,
        [
            "not attached: @./x (no such file)",
            "not attached: @/x (no such file)",
            "not attached: @~/x (no such file)",
            "not attached: @src/mian.rs (no such file)",
            "not attached: @src is a directory",
        ]
    );
}

#[test]
fn a_pasted_image_is_sent_by_the_name_of_its_attachment() {
    let tree = Tree::new();
    let clip = tree.path("clips/4242.1.png");
    let text = format!("what is wrong in this layout @{} please", clip.display());
    let words = tree.find(&text);
    assert_eq!(words.files.len(), 1);
    assert!(words.files[0].clip);
    assert_eq!(words.files[0].name, "paste-1.png");
    assert_eq!(words.sent_text(&text), "what is wrong in this layout @paste-1.png please");
    assert!(!words.sent_text(&text).contains("clips"));
}

#[test]
fn the_name_of_a_paste_comes_from_its_clip_file() {
    assert_eq!(pasted_name("4242.1.png").as_deref(), Some("paste-1.png"));
    assert_eq!(pasted_name("4242.12.jpeg").as_deref(), Some("paste-12.jpeg"));
    for other in ["shot.png", "4242.png", "a.1.png", "4242.1.", "4242..png"] {
        assert_eq!(pasted_name(other), None, "{other}");
    }
}

/// Limits with chunks of 4 bytes, so a file of 9 bytes goes in three chunks.
fn limits() -> AttachmentLimits {
    AttachmentLimits {
        max_file_bytes: 20 * 1024 * 1024,
        max_per_message: 2,
        chunk_bytes: 4,
        max_edge_px: 2000,
    }
}

fn capabilities(limits: Option<AttachmentLimits>) -> Capabilities {
    Capabilities { attachments: limits, ..hello_result().capabilities }
}

fn prompt(text: &str) -> PromptSend {
    PromptSend {
        command_id: "019a9b1c-3d00-7a10-8b20-0000000000c1".parse().unwrap(),
        conversation_id: None,
        new_conversation: false,
        text: text.to_owned(),
        context: None,
        last_command: None,
        recent_commands: Vec::new(),
        settings: efr_protocol::TurnSettings::default(),
        attachments: Vec::new(),
    }
}

fn sent() -> PromptSendResult {
    PromptSendResult {
        conversation_id: conversation(),
        turn_id: turn(),
        seq: Seq::new(10),
        queued: false,
        settings: None,
        route: None,
    }
}

fn finished(name: &str, bytes: u64) -> UploadInfo {
    UploadInfo {
        name: name.to_owned(),
        media_type: "application/octet-stream".to_owned(),
        kind: efr_protocol::AttachmentKind::Other,
        bytes,
        sent: None,
    }
}

/// Answers the puts of one file whole as efrd does, chunk by chunk, and returns their
/// upload id, offsets, sizes and origins.
async fn take_file(conn: &mut Conn, total: u64) -> (UploadId, Vec<(u64, usize)>, AttachmentOrigin) {
    let mut chunks = Vec::new();
    loop {
        let (id, method) = conn.request().await;
        let Method::AttachmentPut(put) = method else {
            panic!("expected attachment.put, got {}", method.name());
        };
        let received = put.offset + put.data.as_bytes().len() as u64;
        chunks.push((put.offset, put.data.as_bytes().len()));
        let upload = (received == total).then(|| finished(&put.name, total));
        conn.reply(id, &AttachmentPutResult { received, upload }).await;
        if received == total {
            return (put.upload_id, chunks, put.origin.unwrap());
        }
    }
}

async fn client(ctx: &crate::context::Context) -> Client {
    ctx.connect(efr_protocol::Origin::Shell, None).await.unwrap()
}

#[tokio::test]
async fn each_file_goes_in_chunks_and_the_prompt_names_the_uploads() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let tree = Tree::new();
    let text = "compare @shot.png with @\"my file.png\"";
    let words = tree.find(text);
    let script = async {
        let mut conn = daemon.accept_with(capabilities(Some(limits()))).await;
        let first = take_file(&mut conn, 4).await;
        let second = take_file(&mut conn, 9).await;
        let (id, method) = conn.request().await;
        let Method::PromptSend(params) = method else {
            panic!("expected prompt.send, got {}", method.name());
        };
        conn.reply(id, &sent()).await;
        (first, second, params)
    };
    let (result, (first, second, params)) =
        tokio::join!(async { send(&ctx, &client(&ctx).await, prompt(text), &words).await }, script);
    assert_eq!(result.unwrap().turn_id, turn());
    assert_eq!(first.1, [(0, 4)]);
    assert_eq!(second.1, [(0, 4), (4, 4), (8, 1)]);
    assert_eq!(first.2, AttachmentOrigin::Path { path: tree.path("project/shot.png") });
    assert_eq!(params.attachments, [first.0, second.0]);
    assert_eq!(params.text, text);
}

#[tokio::test]
async fn a_prompt_with_no_file_is_one_send_also_without_the_capability() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let words = Tree::new().find("explain @src");
    let script = async {
        let mut conn = daemon.accept_with(capabilities(None)).await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::PromptSend(_)), "{}", method.name());
        conn.reply(id, &sent()).await;
    };
    let (result, ()) = tokio::join!(
        async { send(&ctx, &client(&ctx).await, prompt("explain @src"), &words).await },
        script
    );
    assert!(result.is_ok());
}

/// The note of `text` against `limits`, when efr sends nothing for it.
async fn refused(text: &str, limits: Option<AttachmentLimits>) -> String {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let tree = Tree::new();
    let words = tree.find(text);
    let script = async {
        let mut conn = daemon.accept_with(capabilities(limits)).await;
        conn.until_closed().await
    };
    let (result, frames) = tokio::join!(
        async {
            let client = client(&ctx).await;
            let result = send(&ctx, &client, prompt(text), &words).await;
            drop(client);
            result
        },
        script
    );
    assert!(frames.is_empty(), "nothing goes to efrd: {frames:?}");
    match result {
        Err(Failure::NotAttached(note)) => note,
        other => panic!("expected a note, got {other:?}"),
    }
}

#[tokio::test]
async fn a_file_over_a_limit_sends_nothing_and_says_which_limit() {
    assert_eq!(
        refused("what is @shot.png", None).await,
        "not attached: efrd does not take attachments (attachments.enabled, or an efrd before 0.0.13)."
    );
    assert_eq!(
        refused("@shot.png @shot.png @src/main.rs", Some(limits())).await,
        "not attached: 3 files. The limit is 2 for one prompt (attachments.max_per_message)."
    );
    let small = AttachmentLimits { max_file_bytes: 8, ..limits() };
    assert_eq!(
        refused("what is @\"my file.png\"", Some(small)).await,
        "not attached: my file.png is 9 B. The limit is 8 B (attachments.max_file_mib)."
    );
}

#[test]
fn the_size_note_names_the_limit_in_the_number_of_its_key() {
    let tree = Tree::new();
    let mut files = tree.find("@shot.png").files;
    files[0].bytes = 60 * MIB;
    let fifty = AttachmentLimits { max_file_bytes: 50 * MIB, ..limits() };
    let Err(Failure::NotAttached(note)) = checked(Some(&fifty), &files) else {
        panic!("expected a note");
    };
    assert_eq!(
        note,
        "not attached: shot.png is 62 MB. The limit is 50 MB (attachments.max_file_mib)."
    );
}

#[tokio::test]
async fn efrd_refusing_a_file_sends_nothing_and_drops_what_went() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let text = "why @shot.png and @src/main.rs";
    let tree = Tree::new();
    let words = tree.find(text);
    let script = async {
        let mut conn = daemon.accept_with(capabilities(Some(limits()))).await;
        let (first, ..) = take_file(&mut conn, 4).await;
        let (id, _) = conn.request().await;
        let body = ErrorBody::new(ErrorCode::Invalid, "the file of the @path is a secret file")
            .with_data(serde_json::json!({ "secret_file": true }));
        conn.fail(id, body).await;
        let (id, method) = conn.request().await;
        let Method::AttachmentDiscard(discard) = &method else {
            panic!("expected attachment.discard, got {}", method.name());
        };
        assert_eq!(discard.upload_id, first);
        conn.reply(id, &efr_protocol::AttachmentDiscardResult {}).await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::AttachmentDiscard(_)), "{}", method.name());
        conn.reply(id, &efr_protocol::AttachmentDiscardResult {}).await;
    };
    let (result, ()) =
        tokio::join!(async { send(&ctx, &client(&ctx).await, prompt(text), &words).await }, script);
    let Err(Failure::NotAttached(note)) = result else {
        panic!("expected a note, got {result:?}");
    };
    assert_eq!(note, "not attached: src/main.rs is a secret file.");
}

#[tokio::test]
async fn missing_uploads_go_again_with_the_same_ids_and_the_same_command() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let tree = Tree::new();
    let clip = tree.path("clips/4242.1.png");
    let text = format!("why @shot.png and @{}", clip.display());
    let words = tree.find(&text);
    let script = async {
        let mut conn = daemon.accept_with(capabilities(Some(limits()))).await;
        let (shot, ..) = take_file(&mut conn, 4).await;
        let (pasted, _, origin) = take_file(&mut conn, 6).await;
        assert_eq!(origin, AttachmentOrigin::Clipboard);
        let (id, method) = conn.request().await;
        let Method::PromptSend(first) = method else { panic!("expected prompt.send") };
        let missing = MissingUploads { missing_uploads: vec![pasted] };
        let body = ErrorBody::new(ErrorCode::NotFound, "an upload is missing")
            .with_data(serde_json::to_value(&missing).unwrap());
        conn.fail(id, body).await;
        // The clip file stays until the prompt went, so it can go again.
        assert!(clip.exists());
        let (again, ..) = take_file(&mut conn, 6).await;
        let (id, method) = conn.request().await;
        let Method::PromptSend(second) = method else { panic!("expected prompt.send") };
        conn.reply(id, &sent()).await;
        (shot, pasted, again, first, second)
    };
    let (result, (shot, pasted, again, first, second)) = tokio::join!(
        async { send(&ctx, &client(&ctx).await, prompt(&words.sent_text(&text)), &words).await },
        script
    );
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(again, pasted);
    assert_eq!(first.attachments, [shot, pasted]);
    assert_eq!(second.command_id, first.command_id);
    assert_eq!(second.attachments, first.attachments);
    assert_eq!(second.text, "why @shot.png and @paste-1.png");
    assert!(!clip.exists(), "a clip file goes once its prompt went");
}

#[tokio::test]
async fn uploads_that_never_arrive_end_the_prompt_after_three_more_sends() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let tree = Tree::new();
    let words = tree.find("why @shot.png");
    let script = async {
        let mut conn = daemon.accept_with(capabilities(Some(limits()))).await;
        let mut sends = 0;
        let (mut upload, ..) = take_file(&mut conn, 4).await;
        loop {
            let (id, method) = conn.request().await;
            match method {
                Method::PromptSend(_) => {
                    sends += 1;
                    let missing = MissingUploads { missing_uploads: vec![upload] };
                    let body = ErrorBody::new(ErrorCode::NotFound, "an upload is missing")
                        .with_data(serde_json::to_value(&missing).unwrap());
                    conn.fail(id, body).await;
                    if sends < 4 {
                        upload = take_file(&mut conn, 4).await.0;
                    }
                }
                Method::AttachmentDiscard(_) => {
                    conn.reply(id, &efr_protocol::AttachmentDiscardResult {}).await;
                    return sends;
                }
                other => panic!("unexpected {}", other.name()),
            }
        }
    };
    let (result, sends) = tokio::join!(
        async { send(&ctx, &client(&ctx).await, prompt("why @shot.png"), &words).await },
        script
    );
    assert!(matches!(result, Err(Failure::Lost)), "{result:?}");
    assert_eq!(sends, 4);
}

#[tokio::test]
async fn another_refusal_of_the_prompt_drops_the_uploads() {
    let env = TestEnv::new();
    let daemon = env.listen();
    let ctx = env.context();
    let tree = Tree::new();
    let words = tree.find("why @shot.png");
    let script = async {
        let mut conn = daemon.accept_with(capabilities(Some(limits()))).await;
        take_file(&mut conn, 4).await;
        let (id, _) = conn.request().await;
        conn.fail(id, ErrorBody::new(ErrorCode::Busy, "efrd is busy")).await;
        let (id, method) = conn.request().await;
        assert!(matches!(method, Method::AttachmentDiscard(_)), "{}", method.name());
        conn.reply(id, &efr_protocol::AttachmentDiscardResult {}).await;
    };
    let (result, ()) = tokio::join!(
        async { send(&ctx, &client(&ctx).await, prompt("why @shot.png"), &words).await },
        script
    );
    let Err(Failure::Client(ClientError::Server { body })) = result else {
        panic!("expected the refusal, got {result:?}");
    };
    assert_eq!(body.code, ErrorCode::Busy);
}
