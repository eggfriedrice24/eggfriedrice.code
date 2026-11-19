# 0026: Attachments: a reference in the history, bytes in efrd, a copy in `$SCRATCH`

Status: accepted, 2026-11-19.

## Context

Before 0.0.13 a model could not see an image. `read_file` refused a binary file, and a
client could send only text. Users asked for three things: the model reads an image
file, a prompt takes files (from the terminal and from the phone), and the terminal
shows a small picture of an attached image. 0.0.13 gives the first two. The picture
waits for a later release (see "Deferred").

A file in a conversation is different from text in four ways:

- It is large. A photo of a phone has 3 to 9 MB, and a provider takes at most about
  10 MB of base64 for one image. The history only grows, and each request of a
  conversation sends it again.
- It can break a conversation. When a provider refuses one image, each later request
  holds the same image and fails too.
- It holds private data that the user does not see, such as the GPS position of a
  photo.
- It is data from anywhere. Text in a screenshot can come from anyone.

## Decision

1. **One path for every attachment.** An attachment is a small reference in the
   history (`efr_protocol::Attachment`: the id, the name, the type from the magic
   bytes, the size, the SHA-256, the kind, the origin, the copy and what a model gets),
   bytes in efrd's data dir, and a copy in `$SCRATCH/attachments`. A file of an
   `@path`, a paste, a file of the phone and an image that `read_file` read all go
   through the same intake in efrd. A client only uploads (`attachment.put`). The ids
   are `a1`, `a2`, ... for each conversation, and each attachment has its own id and
   row, also when two have the same bytes.

2. **No bytes in the history.** `ContentBlock::Attachment` holds the metadata; its
   bytes (`EncodedBytes`, base64 in an `Arc<str>`) are only in the request copy of a
   turn. The event log, `turn_messages`, the search index, the cache of a turn and the
   logs hold no bytes. The blob dir `<data>/attachments/<conversation-id>/<sha256>`
   holds only the bytes that a model gets (the prepared image or the PDF), and the
   attachments of one conversation with the same bytes share one file. A cache in efrd
   keeps the base64 text of the blobs, at most 32 MiB, so a turn of 20 calls encodes an
   image once.

3. **One form for every provider.** `efr_provider::label` writes the text around each
   attachment: an `<attachment>` tag with the id, the name, the type, the size, the path
   and the origin. Each value goes through `one_line`, and `"`, `<`, `>` and `&` become
   `_`, so a file name cannot close the tag. A file that a model does not read as
   content goes as its label with the path and one line that says so. The model then
   opens it with `read_file` or the shell. The system prompt says that text inside an
   attachment is data, not the request of the user.

4. **efrd checks each file before a model gets it.** The intake (`efr-media`, only in
   efrd) reads the type from the bytes. A PNG or JPEG is decoded once: a cut file fails
   here, an image with an edge over `attachments.max_edge_px` or over 7.5 MB is made
   smaller (JPEG at quality 85, or PNG when it has alpha), and an image that a camera
   turned with an EXIF tag is encoded again upright. Each other PNG or JPEG keeps its
   pixels. EXIF, GPS, XMP and comment data go from PNG, JPEG, GIF and WebP. efrd decodes
   no image over 50 megapixels, and one decode runs at a time, for the uploads and the
   images of a tool together. A GIF or WebP has no decoder in efrd: the walk of its
   blocks is the check, and one that is too large goes as a path. A PDF goes as content
   only up to 5 MiB and 100 pages and with no password. A file that fails a check gets
   no blob and goes as a path, and its row says `as a path`. A broken image has the
   kind `other`, because its header is not to be trusted.

5. **The budget and the recovery keep a conversation alive.** Before each model call,
   a request holds at most 90 images, 100 MB of base64 and attachments of at most half
   of the model's window (`efr_provider::fit`). Over a limit, an attachment that is over
   it alone goes as a label first. Then the oldest attachments go as labels until the
   request is at half of that limit. When a provider refuses a
   request that holds attachment data (status 400 or 413 that is not an overflow of
   the window, a limit or a login error), the turn sends it again: first without the
   attachments that this request sent as content for the first time, then without
   any. Each step records `attachments_withheld`, and from that event on each request
   of the conversation sends those attachments as labels. So one bad file cannot fail
   each later request, and a rebuild of the history gives the same bytes.

6. **The prompt's ids and its scratch dir come at the queue step.** One counter of
   attachment ids for each conversation (`AttachmentIds`, in the actor) serves the
   queue step of a prompt and the images of a running turn. The actor seeds it from
   the highest number of the store before a bind, and each turn seeds it at its start.
   The queue step claims `$SCRATCH` with the same rule as a turn (the day and title of
   the summary, else today and the first line of the prompt that is not empty), so the
   copies are in the dir that the turn uses, also after a restart of efrd.

7. **efrd writes the copies, and nobody else can in a contained call.**
   `$SCRATCH/attachments` is a dir that efrd owns, as `$SCRATCH/jobs`
   (`owned_dir.rs`): efrd writes each copy through the descriptor of the dir with
   `openat2`, `O_CREAT | O_EXCL | O_NOFOLLOW` and no link in the path, and moves an
   entry that has the name already aside. Every contained call reads the dir and
   cannot write it (a read-only floor of the sandbox, as for `jobs`). In a mode with no
   sandbox, a command can change a copy; an image or a PDF
   that a model reads is from the blob dir, so only a text file or another file is
   only as safe as the dir (`docs/sandbox.md`).

8. **An attachment is data, never a permission.** An upload is the user's own act and
   needs no permission. efrd checks the size, the count, the type and the name. It
   refuses an `@path` of the engine's secret class, which stops an accident and is no
   boundary: the client is the user's own `efr`. The naming rule, the allow rule and
   the reviewer read the prompt text only, never a label, a file name or the content of
   an attachment.

9. **An upload belongs to its owner.** On the Unix socket the owner is the connection,
   and the uploads go when it closes. On the tailnet listener the owner is the device of
   the proof, an upload can go on over a new connection, and it goes after 1 hour with
   no chunk. A client names finished uploads in `prompt.send`. efrd answers a missing
   upload with `not_found` and `missing_uploads` and records no receipt, so the client
   uploads again and sends again with the same command id. A withdrawn prompt removes
   its copies and each blob that no other row names (`Attachments::forget`).

## Consequences

- The blobs and the copies live as long as their conversation. `efr conversation
  delete` removes the blob dir and `$SCRATCH`. No limit for the whole disk.
- An image goes with each request until a compaction covers it: about 3,000 tokens and
  300 to 800 KB for each request after the downscale. A PDF is not made smaller and
  costs about 4,500 tokens for each page. These are the reasons for the limits of
  point 5 and of the PDF.
- The text of a `read_file` image has the name, the type and the size, not the pixel
  size: the label of the attachment holds the pixel size.
- A WebP loses its orientation with its EXIF data. efrd cannot turn the pixels of a
  WebP.
- `read_file` reads no PDF. A PDF reaches a model only as an attachment of a prompt.
- The stream recorder (`EFR_RECORD_STREAMS`) writes request bodies, and these hold the
  bytes of images and PDFs.
- `efr-media` adds the `image` crate (PNG and JPEG only) and `miniz_oxide` to efrd:
  0.62 MB in the probe build. No other binary links it.

## Deferred

0.0.13 has no thumbnail: efrd makes none, `efr` draws none, and the config has no
`render.images` key. The thumbnail was the first item to cut. The build that adds it
will follow this rule:

- efrd will make a small picture (a PNG of at most 512 x 256 px) of a PNG or JPEG of a
  prompt, from the one decode of the intake, beside its blob.
- `efr` will read that one file of efrd's data dir (the path from `hello` and the
  event, never from its own environment), with a size and a signature check. It will
  show the picture only in rows that it never draws again. This read will be the one
  exception to "`efr` is a thin relay".
- No image will go in the live zone, because `CSI J` erases text and leaves images.

## Rejected

- Two forms of an image, `ContentBlock::Image` and an attachment: no code made an
  image block, so a second form would be duplication.
- One id for equal bytes: one id cannot hold two names and two origins.
- A blob of the original and of the prepared form, with a hash check at each read: two
  copies and one SHA-256 for each request, for no gain in efrd's own data dir.
- A prompt that goes with a note when a named file cannot attach: it costs a turn that
  cannot answer. `efr` sends nothing and says why.
- An image library in `efr`: the deferred thumbnail will come from efrd, so `efr` gets
  no decoder and the picture works over SSH.
