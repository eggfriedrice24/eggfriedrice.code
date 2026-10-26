# Subscribe recordings

These files are what a phone gets from `conversation.subscribe` with `drafts: true`
during a scripted turn of a test daemon. The replay tests of the app read them.

Do not edit these files. `just bless` writes them again
(`recordings::bless_recordings` in `crates/efr-daemon/tests/it/recordings.rs`). Read the
diff before you commit it.

## Format

- Each line is one JSON object: one `ConversationSubscribeItem`, the `item` of a server
  item frame. Its `kind` is `snapshot`, `event` or `draft`.
- The first line is the snapshot. The recording stops after `turn_completed`.
- Offsets of drafts and of `assistant_message_updated` count bytes of UTF-8.
- The test paths are replaced: `/home/you` is the home directory, `/home/you/project` is
  the working directory and `/efr-test` is the root of the other test directories.
- The time starts at `2026-10-04T12:00:00Z` and moves 50 ms for each delta of the model.

## How the recordings are made

The model sends one delta at a time. The next delta goes only after the draft of the
last one came, so each delta is a draft of its own, as on a fast link. The turn records
an `assistant_message_updated` when 200 ms of test time went by. The order of a draft
and an event that come at the same time can change from one run to the next. The test
of each recording checks that the drafts, the updates and the snapshot build each
completed text with no gap, and that the events match the committed file.

## Scenarios

| File                            | What it shows                                                                                                                                                                             |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `long_reply.ndjson`             | About 20 KB of Markdown prose in deltas of four words.                                                                                                                                    |
| `code_blocks.ndjson`            | Code fences in Rust, TypeScript, Bash and with no language, cut in deltas of 9 characters.                                                                                                |
| `tables.ndjson`                 | Two GFM tables with alignment, cut in deltas of 7 characters.                                                                                                                             |
| `multibyte.ndjson`              | CJK, Korean, Greek, Cyrillic, Hebrew, Arabic, emoji with skin tones, families, flags and keycaps, and a combining mark, cut in deltas of 2 code points. A delta can end inside one emoji. |
| `reasoning.ndjson`              | Reasoning in two sections with bold titles, then the answer.                                                                                                                              |
| `tool_output.ndjson`            | Text, a `read_file` call whose input streams as `tool_input` drafts, its output, then the answer as message 1.                                                                            |
| `approval_in_the_middle.ndjson` | A `write_file` question that the phone allows (with a diff), then a `sudo` question with `local_only: true`: the daemon refuses the phone's answer and the terminal denies it.            |
| `join_mid_reply.ndjson`         | The phone joins in the middle of a long reply: the snapshot has `live` with the text and the reasoning so far, and the drafts go on past it.                                              |

A `tool_call_output_updated` event is not in these recordings: it needs a real shell.
