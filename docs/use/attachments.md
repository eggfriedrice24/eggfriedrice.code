# Attachments

<!-- sources: crates/efr-cli/src/attach.rs, attach/upload.rs, format/attach.rs, error.rs (NotAttached, UploadLost); shell/zsh/efr.plugin.zsh (_efr_attach_colour, _efr_prompt_line, _efr_paste_line, _efr_paste_wanted, _efr_terminal_allows, _efr_clip_event); crates/efr-daemon/src/attachments.rs, attachments/intake.rs, attachments/uploads.rs, prompt_file.rs, methods/attachment_put.rs; crates/efr-provider/src/attachment.rs (label, fit); docs/config.md [attachments]; docs/adr/0026-attachments.md, 0027-a-terminal-read-for-a-paste.md -->

An attachment is a file that goes with a prompt, such as a screenshot, a PDF or a CSV
file. A model that reads images or PDFs gets the content. Each other file goes as its
path, and the model opens it with a tool.

You attach a file in three ways:

- with an `@path` word in a `,` line (this page),
- with a paste of an image into a `,` line in Ghostty 1.4 or later (this page),
- with the plus button of the phone app (see [The phone app](phone.md#attach-photos-and-files)).

## Attach a file with `@path`

Put `@` and the path of the file in a `,` or `,new` line:

```sh
, why is the header cut off in @docs/shot.png
```

The rules of an `@path` word:

- The word starts with `@` at the start of the prompt or after a blank.
- The path is absolute, starts with `~/`, `./` or `../`, or is relative to the
  directory of your shell.
- Quotes work: `@"my file.png"` and `@'a b.png'`. A backslash before a character stands
  for that character, so `@/x/new\ logo.png` names `new logo.png`. Ghostty writes a
  dropped file in this form.
- When a word names no file, efr tries it once more without the marks `.,;:!?` at its
  end. So `what is in @shot.png?` attaches `shot.png`.
- `user@host` is never a word.
- The prompt text keeps each word as you typed it. The model gets the text and the
  attached files.

A word that names no file stays text, and the prompt goes. efr prints a note only when
the word looks like a path:

- `not attached: @src is a directory`, always for a directory.
- `not attached: @x (no such file)`, when the word starts with `/`, `~/`, `./` or
  `../`, or when it has a `/` and its parent directory exists.

Words such as `@alice`, `@types/node`, `@app.route` and `@v1.2.3` print nothing.

A prompt never goes without a file that it names. When a file cannot attach, efr sends
nothing and exits with code 2:

| Text | Why |
|---|---|
| `not attached: shot.png is 34 MB. The limit is 20 MB (attachments.max_file_mib).` | The file is too large. |
| `not attached: 12 files. The limit is 10 for one prompt (attachments.max_per_message).` | The prompt names too many files. |
| `not attached: efrd does not take attachments (attachments.enabled, or an efrd before 0.0.13).` | efrd takes no files. |
| `not attached: <path> (<reason>)` | efrd refused the file, such as a secret file, or efr cannot read it. |

The line stays in the history of zsh, so you can fix it and send it again.

efr uploads each file in chunks before it sends the prompt. When efrd lost an upload
(a restart of efrd between the upload and the send), efr uploads the file again and
sends again, at most 3 times. Then it prints `the upload did not arrive`.

A steer takes no file: an `@path` word in a `,!` line or in the input row of a running
turn stays text, with the note `attachments go with a prompt, not with a steer`.

In zsh, each `@path` word of a `,`, `,new` or `,!` line, and of a line in sticky mode,
that names a regular file shows in the accent colour while you type. The line has no
link. Tab after `@` completes a file name on `,`, `,new` and `,!` lines.

## Paste an image

In Ghostty 1.4.0 or later, a paste into a prompt line attaches an image of the
clipboard. Copy the image, type `, ` and press the paste key. The line gets `@` and the
absolute path of a clip file, and a blank. The prompt then names the image
`@paste-1.png`, and efr removes the clip file after the prompt went.

- The paste works on a prompt line: `,`, `,new`, `,!`, and each line of sticky mode
  that does not start with `!`, also an empty one. On each other line, zsh pastes as
  before. On a `,!` line the word stays text, because a steer takes no file.
- When the clipboard has an image and text, the image goes. Text goes into the line as
  before. Text over 16 KiB goes as a `.txt` file.
- When a paste adds nothing, one line under the prompt tells you why. It goes at the
  next key.

| Text | Why |
|---|---|
| `efr: the clipboard image is 34 MB. The limit is 20 MB (attachments.max_file_mib). Nothing was added.` | The image is too large. |
| `efr: the terminal sent no clipboard data in 3 s. Nothing was added.` | The terminal did not answer. |
| `efr: the clipboard read stopped. Nothing was added.` | The reply of the terminal stopped. |
| `efr: the clipboard read took more than 30 s. Nothing was added.` | The reply took too long. |
| `efr: the clipboard read failed again. Pastes on , lines are text again in this shell.` | The second failed read in this shell. |
| `efr: the terminal does not allow clipboard reads (clipboard-read). Pastes on , lines are text again in this shell.` | Ghostty has `clipboard-read = deny`. |
| `efr: the clipboard has no image and no text.` | Nothing to paste. |

`attachments.paste` sets when a paste attaches:

- `auto` (the default): Ghostty 1.4.0 or later (`TERM_PROGRAM_VERSION`), never inside
  tmux, GNU screen or zellij, and only when stdout is a terminal.
- `on`: every terminal.
- `off`: never.

With `attachments.enabled = false`, the paste is off too.

In another terminal, or inside tmux, type `@` and drop the file on the window: the
terminal pastes its path. A drop also works while the paste of images is on.

How efr reads the clipboard is in [ADR
0027](https://github.com/eggfriedrice24/eggfriedrice.code/blob/main/docs/adr/0027-a-terminal-read-for-a-paste.md):
efr reads the terminal only to answer your own paste, with deadlines, and the keys that
you type during the read come back to the line.

## The row of a prompt

A prompt with attachments gets one row at the start of its turn, and `efr history`
shows it too. Each file shows its kind (`image`, `pdf`, `text` or `file`), its name and
its size, and an image also shows its pixels. The name links to the copy of the file.
[The turn view](turn-view.md) shows the row and tells when it says `as a path`.

## What a model reads

| File | A model that reads it | Each other model |
|---|---|---|
| PNG, JPEG, GIF, WebP | the image | the path |
| PDF | the document, up to 5 MiB and 100 pages and with no password | the path |
| A text file, each other file | the path | the path |

- A model of the ChatGPT plan reads no PDF.
- efr makes a large PNG or JPEG smaller (`attachments.max_edge_px`, default 2000 px),
  and removes EXIF, GPS, XMP and comment data from each image. A GIF or WebP that is
  too large goes as a path.
- A cut or broken image, an image over 50 megapixels and a PDF that a provider would
  refuse go as a path. The row of the prompt says `as a path`, and a broken image shows
  as the kind `file`.
- A HEIC photo goes as a path from the terminal. The phone app makes a photo into a
  JPEG first.
- When a provider refuses a file, efr sends the request again with the path of that
  file, and later requests send the path too. One row tells you, such as
  `not sent: shot.png. The provider refused it; the model gets the path.`

[Providers](../providers.md#images-and-pdfs) tells what each backend reads, and the
limits of one request.

The model gets a label around each file, with its id, name, type, size and path. For a
text file or another file of an `@path`, the path is your file, and the label also names
the copy, which is the file as it was when you sent the prompt.

## Where the files go

- efrd keeps a copy of each file in `$SCRATCH/attachments/<id>-<name>`, such as
  `a1-shot.png`, with the bytes that you sent. You, the model and the hidden shell can
  open it. A contained call can read the dir and cannot write it.
- efrd keeps the bytes that a model gets (the prepared image or the PDF) in its data
  dir. The event log and the search index hold only the name, the type, the size and
  the path, never the bytes.
- `efr conversation delete` removes the copies and the bytes. A prompt that you
  withdraw before it runs removes its files too.
- [Storage](../storage.md) gives each path.

## Privacy and safety

- An attachment is data, never a permission. Text in an image or a file can come from
  anyone, so the reviewer and the rules of efr read only the text that you typed.
- efrd refuses an `@path` of a secret file, such as a file in `~/.ssh` or
  `~/.aws/credentials` (the class "Secrets" of [Permissions](../permissions.md)). This
  stops an accident. It is no boundary: efr is your own client.
- The provider of the model gets the content of an image or a PDF that the model reads.
  A file that goes as a path stays on your machine until the model opens it with a
  tool.
- In a mode with no sandbox, a command can change a copy in `$SCRATCH/attachments`. An
  image or a PDF that the model reads comes from efrd's own copy, but a text file is only
  as safe as that dir ([The auto sandbox](../sandbox.md)).

## The limits

`[attachments]` in [`config.toml`](../config.md#attachments) sets them:

| Key | Default | What it sets |
|---|---|---|
| `enabled` | `true` | `false`: efrd takes no file, an `@path` stays text, and `read_file` refuses an image |
| `max_file_mib` | `20` | the largest file, from 1 to 50 MiB |
| `max_per_message` | `10` | the most files of one prompt, from 1 to 20 |
| `max_edge_px` | `2000` | the longest edge of an image that a model gets |
| `paste` | `"auto"` | the paste of an image (above) |

A larger `max_file_mib` does not send a larger PDF as content: that limit is 5 MiB.
