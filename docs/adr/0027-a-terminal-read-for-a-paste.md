# 0027: A terminal read for a paste

Status: accepted, 2026-11-19.

## Context

The user wants to paste a screenshot into a `,` line, as in a chat app. A terminal
paste gives text only. Ghostty 1.4 has a paste event (OSC 5522): while a program turns
on mode 5522, the paste key sends an event with the list of the clipboard's types and a
password, and the program then asks for the data that it wants.

Two facts make this hard:

- efrd has no terminal. Only a process on the user's tty can read the reply.
- zsh alone cannot read a large reply. `read -k` takes one character at a time, and zsh
  has no base64 decoder. An image of 20 MiB is 27 MB of base64.

efr has a rule: "efr never asks the terminal" (`crates/efr-cli/src/terminal.rs`). A
query needs a reply on stdin, and a reply that comes late is typed into the line of the
shell. The keys that the user types during a query are lost or mixed with the reply.

## Decision

1. **The paste reads the terminal, and nothing else does.** The one exception to the
   rule is a read that answers the user's own paste. The terminal frames the reply: it
   ends with a packet with the status `DONE` or an error.

2. **ZLE takes the small event, and a hidden `efr clip` reads the data.** The plugin
   turns mode 5522 on only while the line is a prompt line (`,`, `,new`, `,!`, or a
   sticky line that does not start with `!`), and off at each other line, before a
   command runs and when the shell exits. So no program that the user runs gets the
   mode. A widget bound to `ESC ] 5522 ;` reads the event with `read -k`, at most 8 KiB
   and 1 s. `efr clip` then opens the tty itself, asks for the image or the text with
   the password, decodes the base64 chunk by chunk and writes a clip file. The line
   gets `@<clip file> `, and `efr` sends the word as `@paste-<n>.<ext>`.

3. **Deadlines, and no byte left in the tty.** `efr clip` waits 3 s for the first byte,
   1 s between two bytes and 30 s in all. It never exits in the middle of a reply,
   because the bytes that it leaves would be typed into the line. Over the size limit
   or the 30 s, it reads and drops the rest to `DONE`, or to 0.5 s of quiet, then
   prints the error. A packet that is not a paste event, such as a reply that came too
   late, goes to `efr clip --drain`, which drops it.

4. **Typed keys come back.** The keys that the user types during the read go back to
   the line with `zle -U`, in their order.

5. **The password stays out of argv.** The event goes to `efr clip` in the environment
   (`EFR_CLIP_EVENT`), never in its arguments, as each other text of the user.

6. **A broken read cannot break each paste.** After `EPERM` (the terminal does not
   allow clipboard reads) and after two failed reads in one shell, the plugin turns the
   mode off for the rest of that shell, with one message. Pastes on prompt lines are
   text again.

7. **The gate.** `attachments.paste`: `auto` means Ghostty 1.4.0 or later, outside
   tmux, GNU screen and zellij, with stdout a terminal; `on` means every terminal;
   `off` means never. efrd writes `paste off` to the prompt file of the plugin when
   `attachments.enabled` is false. The plugin shares one gate function with the
   desktop notice.

## Consequences

- An image on the clipboard wins over text. Text over 16 KiB becomes a `.txt` clip
  file, because one environment variable takes at most 128 KiB.
- The clip files are on disk below the state root (`<state>/clips/<shell pid>.<n>.<ext>`,
  0600, dir 0700), not in the runtime dir, which is a tmpfs. At most 10 for each shell.
  `efr` removes a clip after its upload, and each `efr clip` removes clips older than 1
  day, so the plugin needs no cleanup.
- In other terminals the user types `@` and drops the file on the window.
- The input row of a running turn drops each OSC, DCS and APC string, so a late reply is
  never typed into it.
- The answer of `efr clip` to the plugin has five kinds, not three: `file` (an image,
  or text over 16 KiB), `text`, `error`, `failed` (a read that counts toward the two
  failures) and `off` (the mode goes off). The plugin must know which errors count,
  which turn the paste off, and that large text is a file too. `efr clip` also takes
  `--shell <pid>` for the name of the clip file.
- A paste with the middle mouse button (`loc=primary`) is asked for again with
  `loc=primary`. Ghostty did no primary paste in the probe of 0.0.13.

## Rejected

- A read in zsh: too slow for a large image, and no base64 decoder.
- A read in efrd: efrd has no terminal.
- The mode on at every line: each program that the user runs would get paste events
  that it does not know.
- The warm test of the plugin as the gate: it needs a character after `, `, so the mode
  would be off on the most common place for a paste.
