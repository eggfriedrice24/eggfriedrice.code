<!-- sources: docs/config.md [remote]; docs/protocol.md "Remote devices"; docs/permissions.md (the phone rules); crates/efr-config/src/tables/remote.rs; crates/efr-cli/src/cli.rs (DeviceCommand, DeviceAddArgs); crates/efr-cli/src/commands/device.rs; crates/efr-cli/src/error.rs (the device errors and hints); crates/efr-daemon/src/pairing.rs; crates/efr-daemon/src/remote/listener.rs, tailnet.rs, admission.rs, prove.rs, live.rs; crates/efr-daemon/src/error.rs (RemoteOff, NotListening); crates/efr-transport/README.md "ws"; app/src/features (the screens and their texts); app/src/features/machines (the connection of each machine); app/src/features/conversation/model/outbox.ts (the outbox of the prompts); app/src/features/conversations/conversations-queries.ts (the watch and the poll of the lists); app/modules/efr-device-key/README.md; docs/adr/0022-remote-listener-and-device-keys.md -->

# The phone app

The efr phone app lets you follow your conversations on an iPhone, send a prompt and
answer the questions of the agent. The phone connects to efrd directly over your
Tailscale network. There is no relay and no server of a third party.

> The app is a preview. It is not in the App Store. You build it yourself from the
> `app/` directory of the efr repository, with an Apple Developer account and EAS
> Build. The
> [README of the app](https://github.com/eggfriedrice24/eggfriedrice.code/blob/main/app/README.md)
> tells how, with the steps of EAS and the dev, preview and production variants of the
> app, and what works in the app now. The app ran as a dev build on an iPhone with iOS 26:
> the pairing, the chat, the approvals, steer and queue, and the conversations sheet
> work.

## What you need

- Tailscale on the computer that runs efrd: tailscaled 1.100 or later, logged in and
  connected. tailscaled must use a TUN device. efrd does not listen when tailscaled
  runs with `--tun=userspace-networking`.
- Tailscale on the iPhone, logged in as the same Tailscale user. efrd refuses a phone
  of another user, a phone that another tailnet shares with you, and a tagged device.
- A computer that is not a tagged device. A tagged computer belongs to no user, so
  efrd refuses every phone.
- A tailnet policy that lets the phone reach the computer on the port of efrd (7419
  by default). The default policy of Tailscale lets each device of a user reach the
  other devices of that user.

efrd only reads from tailscaled: the status of this machine, the identity of each
peer, and the changes of the network. It does not change your Tailscale settings.

## Turn on the listener

The listener is off by default. Turn it on and restart efrd:

```sh
efr config set remote.enabled true
systemctl --user restart efrd
```

efrd then listens on the Tailscale IPv4 address of this machine (an address in
`100.64.0.0/10`), and on no other address. It does not listen on your LAN, on
loopback or on the internet. The log of efrd shows the address:

```sh
journalctl --user -u efrd | grep 'remote listener'
```

`daemon.json` in the runtime directory also holds it, as `tailnet_endpoint`.

The `[remote]` table of `config.toml` has three keys. Each one needs a restart of
efrd:

| Key | Default | What it does |
|---|---|---|
| `enabled` | `false` | `true` turns on the listener. |
| `port` | `7419` | The TCP port on the Tailscale address, from 1024 to 65535. |
| `tailscaled_socket` | `/var/run/tailscale/tailscaled.sock` | The LocalAPI socket of tailscaled. Change it only when your tailscaled uses another path. |

[config.toml reference](../config.md#remote) has the details.

When Tailscale stops, efrd closes the phone connections and waits. When Tailscale
comes back, or the address or the tailnet of this machine changes, efrd listens again
on the new address. You do not restart efrd for this.

The agent cannot turn on the listener for you. The settings tool counts a change of
`remote.enabled` or `remote.tailscaled_socket` as a change that loosens your
permissions, so it always asks you first. A turn from the phone cannot change
settings.

To turn off the listener, set `remote.enabled` to `false` and restart efrd.

## Pair a phone

Pair each phone once. Run this in a terminal on the computer:

```sh
efr device add
```

efr shows a QR code below the command line:

```text
Scan this code with the efr app on your phone:

  (the code)

The code is good for one phone until 2026-10-26 14:05:00 CET (in 5m 0s). Ctrl+C stops the pairing.
```

On the phone, open the app. When no machine is paired, the app opens on the first step
of the pairing, which shows this command. Tap "Scan code" and point the camera at the
code. The app first checks that the code comes from this efrd. Then it makes the device
key and sends the public key with the code. iOS asks for the camera on the first scan.

The terminal then shows the facts of the phone and asks you:

```text
A phone wants to pair:
  name      Ana's iPhone
  tailscale ana-iphone (ana@example.com)
  key       SHA256:0ed3a6ab957ff6f5
Compare the key with the one that the app shows. Pair this phone? [y/N]
```

The app shows "Confirm on" with the name of the machine, and under it the name of the
phone and its key. Make sure that the key in the terminal is the same as the key that
the app shows. Then press `y`.
efr prints:

```text
paired the device 0193f2a1; efr device revoke 0193f2a1 takes its access away
```

Any other key refuses the phone, and efr prints `nothing was paired`.

The rules of a pairing:

- The code holds a one-time token. It is good for one phone and for 5 minutes.
  efrd keeps only a hash of it, and only in memory. A restart of efrd ends every
  pairing.
- Nothing is paired until you press `y` in the terminal.
- After too many wrong tokens, efrd ends the open pairings.
- Ctrl+C ends the pairing. The token is then good for nothing.
- `efr device add` needs a terminal for its question. In a pipe it stops before it
  opens a pairing.

Options:

- `--name NAME` gives the device a name of your own in `efr device list`. Without it,
  efr uses the name that the phone sends.
- `--invert` swaps the dark and the light parts of the code, for a terminal with a
  light background.

When the window is narrower than the code, efr says how many columns it needs. Make
the window wider.

## The device key

The app makes a P-256 key in the Secure Enclave of the iPhone. The private key never
leaves the phone, and it is not in a backup. efrd keeps only the public key.

On each connection, efrd sends a new challenge, and the phone signs it with its key.
The signature also holds the Tailscale node of the phone and the tailnet of efrd. So a
signature that worked once does not work again, and it does not work from another
device. Until the phone proves its key, efrd answers only the calls of the pairing and
of the proof.

The key needs no Face ID. The app proves the key each time that it connects again.

## Pair more machines

The app can pair with more than one computer, and it connects to all of them at the
same time. To pair another machine, open Settings and tap "Pair a new machine", or
pick "Pair a new machine" in the machine menu of the blank root. Then run
`efr device add` on that machine and scan its code. Each machine gives the phone its
own device key.

## Use the app

After the pairing, the app connects on its own.

### The blank root

The app opens on a blank conversation on the machine that you used last. The title in
the bar is the name of the machine. The status line of the composer shows `~` and the
mode of the new conversation. Type a prompt and tap Send: efrd starts a new
conversation on that machine, and the app opens it. The blank root stays under it.

The menu at the left of the bar changes the machine of the blank root. It shows each
paired machine, and the machines that are offline say so. The bar of each screen has
Settings and New (`+`) at the right. New goes back to the blank root.

### Find a conversation

- Tap the title of the bar. A sheet opens with the conversations of every machine:
  "Needs you" (they wait for you) and "Running" come first, then Today, Yesterday,
  This week and Older. Each row shows the title, the status, the machine, the last
  folder of the directory and the time of the last activity. The times move once a
  minute while the sheet is open. Pull the sheet up to see more. The search bar at the
  bottom finds a conversation by its title, its machine or its directory, and then asks
  efrd. The words that you search for are bold in the titles. The button next to it (a
  pencil) starts a new conversation.
- Tap a row to open the conversation. The sheet closes.
- Touch and hold a row of the sheet for Open and Copy ID.
- The status of a row tells who must act:

  | Status | Meaning |
  |---|---|
  | needs approval | An approval waits. You can answer it here. |
  | needs an answer | A question of the model waits. You can answer it here. |
  | answer in the terminal | Only your computer can answer: a command that needs a password, a question that only the terminal answers, or a question of the model on an efrd that the app cannot answer. |
  | running | A turn runs. |
  | ✗ failed | The last turn failed. Its status is red, and the row stays in its day. |

  The first three have the yolk "!" mark and stand in "Needs you". The phone shows
  "needs an answer" only on an efrd that sends the model's questions (0.0.12 or
  later).
- When a machine does not answer, its rows stay from the last list that the phone
  keeps. They are quieter and say "offline" after the machine, and they show no status,
  because it can be old. A banner at the top of the sheet, with the `wifi.slash`
  symbol, says which machines are offline and how old their rows are:
  `homelab offline · rows from 2h ago`. It names two machines at most
  (`homelab and laptop offline`), else their count (`3 machines offline`). The age is
  the oldest of the kept rows: `12m ago` (at least `1m ago`), `3h ago`, `yesterday`,
  `Nov 12` or `Nov 12, 2025`. With no kept rows of the machine, the banner gives no
  age. The phone keeps no transcript, so an offline conversation can open empty.

The app keeps at most 8 conversations on its stack. A conversation that is on the stack
opens where it is. Touch and hold the back button to see the stack.

### Follow a conversation

A conversation shows your prompts, the replies of the model, its reasoning, the tools
that it runs, the edits with their diff, and a line at the end of each turn. A turn
that runs now comes at once with the text that the model has sent so far, and the rest
streams in.

- Tap a reasoning row or a tool block to open it.
- Touch and hold a message to copy its text. A code block has its own copy button.
- When the agent searched the web, the end of the turn lists the pages that the answer
  cites (`[1]` and the page), and the end line counts the searches. "stored copy" there
  means that the provider searched its own copy of the web, which can be some days old.
  Under the first such turn, one quiet line says it, and what gives the live web, in the
  words of the terminal: `,mode auto`, or with `web.search = "cached"` the setting.
- The app asks before it opens a link. A web page then opens in the app. An image in
  a reply shows as a link, and HTML shows as text, so the app loads nothing that you
  did not tap.
- Near the end, the list follows the new text. When you are more than one screen from
  the end, "Jump to latest" shows above the composer. Tap it to go back to the end.
- At the top, the app gets the older turns from efrd. It keeps them on the phone, so
  they are there when you open the app again.
- A failed turn shows the reason, with "retry" and "copy error". Retry sends the prompt
  of the turn again.
- When the account of a turn reached a limit or has no credit left, the turn end says
  so, with the time of the reset when efr knows it: `the ChatGPT plan reached its usage
  limit · resets 14:20`. Under the newest turn, the app offers the same model on each
  other account that can serve it, such as "switch to ChatGPT plan · b". A full account
  whose limit resets first says when it runs again, such as "b runs again 14:20". The
  app never switches by itself. A tap opens the model sheet with that model on that
  account selected; after you confirm, the offer becomes "retry". An account that is
  signed out gets one quiet line with the command that logs in to it on the machine.
  When each other account of the model is full for longer, the line says when the first
  of them has room again, such as "no other account has room for GPT-6.1 Sol before
  16:05". When no other account runs the model, the line says so.

### Send a prompt

The composer floats over the end of the transcript. At rest it is a one-line pill with
the send button. Tap it to write: it grows to the full composer, with the status line
on top and room for 6 lines. When you tap the transcript or close the keyboard, it
becomes a pill again and keeps your text.

The plus button at the left (attach a file or an image) and the microphone before the
send button (record a voice message) are off for now. A later release turns them on.

The status line shows the directory and how full the context of the model is, such as `ctx 42%`. It shows
"waiting for you" while a question of this conversation waits, and "reconnecting" while
the app connects again. At its right end are the mode of the next message and the model
button. When the line is full, the directory gives way first, then `ctx`.

The mode control shows `cautious`, or `🤖 auto`. Tap it to pick one; the phone keeps the
mode for the conversation. Full mode is never on the phone. Auto from the phone needs
the Face ID unlock of the app (see [Settings](#settings)): while it is off, Auto is
greyed, and the menu has "Turn on Face ID unlock", which opens Settings at that row and
marks it for a moment. While the unlock is on, a message in auto goes at once: the
unlock is the check. A steer adds text to the running turn in the mode of that turn,
also in auto, with or without the Face ID unlock.

When no turn runs, Send sends your prompt. While a turn runs:

- Stop stops the turn.
- When you type, a capsule shows Steer or Queue. "Steer now" adds your text to the
  running turn. "Queue" sends it when the turn ends. Touch and hold the capsule to pick
  one. The app keeps your choice.
- A queued prompt shows "withdraw". Tap it to take the prompt back before it runs.

A prompt shows at once, with "sending", until efrd has it. When the connection stops,
or when iOS closes the app, the app sends the prompt again later. efrd runs it only
once. Send is off while the machine is offline. The app keeps the draft of each
conversation.

### Pick a model

The model button at the end of the status line shows the logo of the provider, the
model of the next message and its effort, such as `Opus 5.5 · high`. When the model
runs on two accounts, the button names the account too: `GPT-6.1 Sol · b · medium`. Tap
it to open the model sheet.

The search bar in the bottom toolbar filters the models as you type: each word must be
in the name or the id of the model. While the search has the focus or text, the sheet
stays at its full height. The filter button next to the search bar picks the provider
(All, Claude, OpenAI) and turns "Legacy models" on or off. A model is legacy
when the machine lists a newer model of its family, such as Opus 5 next to Opus 5.5.
The default model of the machine is never legacy. Legacy models are hidden by default, also from a search. Turn "Legacy models" on in the
filter to see them, under "Legacy" at the end of their account.

Tap the star of a row to make that model the favourite of its account. Each account has
one favourite, and it goes first in its account. A new conversation from the phone
starts with the favourite of the account that you used last.

The sheet lists the models of each account of the machine, in the order of
`efr models`. A header names the account: the kind (`Claude API`, `OpenAI API`,
`ChatGPT plan`), then the label from `[accounts]` or the account id. The default
account of a kind has no id. An account that is signed out has its rows dimmed, and
you cannot pick them: log in on the machine. The check shows what the next message
uses now.

From 80 % of a usage limit, the header of the account shows a warning glyph and the
fullest window, such as `5-hour 92%`. At 100 % it says `limit reached · resets 14:20`,
with the age of the value when it is older than 5 minutes, and its rows dim. "limit
reached" goes away after the reset time. efr learns the limits from the last call of
each account. While a turn runs, the status line shows the limit of its account from
90 %.

- Tap a row to select it. The slider under the list sets the effort of the selected
  model. A dot marks the model's default. A model without efforts has no slider.
- The quiet line under the slider shows the context window and whether the model is
  the default of the machine. "Use default" picks the default again.
- "Use" at the right of the header confirms your pick. It is off until something
  changed. VoiceOver reads what it uses, such as "Use Sonnet 5 · medium", or "Use
  ChatGPT plan · b" when only the account changes. While a turn runs, the quiet line
  says "next message": your pick applies to the next message.
- When the switch compacts the conversation, the sheet says so and the header button
  reads "Switch". "Don't ask again here" stops this warning in this conversation. A
  switch that only starts a cold cache says nothing.

The phone keeps your pick for the conversation, and each prompt from the phone sends
it. The conversation shows one quiet line above the first answer on the new model,
such as `⇄ GPT-6.1 Sol on b · medium · compacted`.

### Answer a question

When the agent asks a question, its tool card in the conversation says "waiting for
approval", and a strip in the composer shows the command with Deny and Allow. Answer
in the strip. When more than one question waits, the strip shows "1 of 3", and the
next question takes its place after your answer. The choices are the same as in the
terminal, except "always allow".

Tap the strip or the tool card to open the full question in a sheet: the whole
command, or the file and its diff, the directory, and why the permission mode asks.
The sheet never opens by itself. Swipe down to close it.

The first answer wins. When you answer a question on the phone, the terminal shows
the answer, and the reverse. After the answer, the tool card says "allowed", "denied"
or "expired". VoiceOver tells who answered and when.

In the last minute before a question expires, the strip and the tool card show the
seconds left. When nobody answers in time, the card says "expired". The call did not
run. When a question closes while its sheet is open, the sheet tells how it ended and
how many more wait, and Next shows the next one.

When another conversation waits for you, a strip under the bar tells you what it waits
for: "Approval waiting", "Question waiting" (a question of the model) or "Answer in the
terminal" (a wait that only your computer can answer, such as a password). Tap it to
open that conversation at the question. When two or more wait, the strip says "2
approvals waiting", "2 questions waiting" or, for a mix, "3 conversations need you",
and opens the sheet with them on top. A yolk dot on the title of the bar tells you too.
efrd sends each change of its list of conversations to the app, so the strip and the
dot show a new question at once. With an older efrd, the app asks every 10 seconds, so
they can come up to 10 seconds late.

### Answer a question of the model

The model can ask you a question with 2 to 4 options. While the app shows the
conversation, the question waits for your answer, as an approval does. The strip in the
composer shows the yolk dot, the question and "Answer", and VoiceOver says "Question"
and the question. When an approval waits too, the older
of the two goes first, and "1 of 2" counts both. Tap the strip: the native action
sheet opens, with the question as its title, each option with its description, then
"Type an answer", "Skip" and Cancel.

- Tap an option to answer with it.
- "Type an answer" turns the composer into an answer field. The row `Answer ·
  <question>` with an xmark stands where the strip was, the field says "Your answer",
  and the hint `Do not type a password or a token here.` stands under it. Send sends
  the answer (VoiceOver reads "Send the answer"). Line breaks become spaces. An answer
  has at most 1024 bytes; past that, Send is dimmed. The text stays in the field until
  efrd takes the answer; when the answer does not arrive, the row says "not sent". The
  xmark leaves the answer field, and your text stays as a draft message. When the
  question ends elsewhere, the field goes back by itself, and the text of the answer
  goes: it never becomes a message.
- "Skip" skips the question. Cancel closes the sheet, and the question stays open.

The row of the question in the conversation says "waiting for you" (a tap opens the
sheet), then how it ended: `answered · <option>`, `answered · "<text>"`, `answered on
<machine> · ...` for an answer from the terminal, "skipped", "nobody answered" or "not
answered · the turn was interrupted". The status line says "waiting for you" while a
question waits. An answer is information for the model: it is no permission, and it
allows no command.

### Auto mode on the phone

In a conversation in `auto`, a reviewer model judges each call that needs more than the
sandbox, and nothing asks you. The tool card shows it with a quiet line in efr's words,
the same words as the terminal:

- "reviewing" while the reviewer judges the call, with what the call asks for under it.
- `reviewer allowed: ...`, then after a colon what the call got, such as
  `github.com:443 reachable, the variable GH_TOKEN`. "host network" (the whole
  network) shows on a line of its own, with a warning glyph.
- `allowed by your rule ...` for a call that one of your always rules covered.
- "refused" and `reviewer refused: <command>: <reason>`, or `efr: <command>` when one of
  efr's own rules refused it. The call did not run, and the model goes on.
- "did not run" and `reviewer: no answer ...` when no reviewer model answered.

When a command changes git settings or another file that runs programs, efr moves the
dangerous ones to quarantine, where nothing reads them. With the reviewer on (the
default), nothing asks you: the card of the command has one quiet line for each file,
such as `.git/config (core.fsmonitor); moved to quarantine` or
`.envrc (a new agent or editor config); left in place`. With the reviewer off, efr asks
whether to keep the changes. The phone shows the question: the card of the command
says "git settings changed", and the strip above the composer asks, such as
`keep .git/config (core.fsmonitor)?`. Tap the strip or the card to see the changes.
The phone can answer Leave: the changes stay in quarantine. Only the terminal can take
a git setting out of quarantine, so the strip shows "Keep on desktop" where the Keep
button would be. After the answer, the card says "kept", "left in quarantine", or "not
answered · left in quarantine" when the question expired or the turn stopped.

After 3 refused actions in a row, or 10 in a turn, one quiet line says that the reviewer
paused for this turn. A pause asks nothing. The next action that needs the reviewer
does not run, and the turn stops with "auto stopped this turn". Send a new prompt that
names what you want.

### When a machine is offline

A line under the bar tells you when the machine of the conversation is offline, or
when Tailscale on the phone is off. The composer then says so too. When the app comes
back to the screen after 10 seconds or more, or when the phone changes its network,
the app connects again at once and gets what it missed.

### Settings

Settings shows each paired machine, with its connection and the version of its efrd.
The menu of a row copies the node name, pairs the machine again, or forgets it.
Under the machines, "Accounts on desktop" lists each account of a machine, named as
in the model sheet, with its fullest limit: nothing below 80 %, the percent from 80 %,
and "limit reached" at 100 %. Tap an account to see each of its limits with its reset
time. An API key shows a limit only when the account hit it.

Settings also has the switch "Ask before a costly model switch" (on by default). Turn
it off, and a tap on a row of the model sheet switches at once, with no confirm.

Under Privacy, the switch "Face ID unlock" (off by default) locks the app. Its name
follows the iPhone: "Touch ID unlock", or "Passcode unlock" when no Face ID or Touch ID
is set up. With it on, efr asks for Face ID, with the passcode of the iPhone as the
fallback, when the app opens and when it comes back after 5 minutes or more in the
background. A lock screen shows nothing of a conversation, and the app switcher shows
only the efr mark. Auto from the phone needs this unlock. The switch asks iOS before it
turns on and before it turns off. When it turns off, each conversation of the phone goes
back to cautious. An iPhone without a passcode cannot turn it on; when you remove the
passcode while it is on, efr turns it off and says why. efr has no passcode of its own.
The app measures the 5 minutes with the clock of the iPhone. A person who sets that
clock back while the app is away can make the time away look shorter than 5 minutes.
Then the app opens with no check.

Settings also has the Haptics switch, which turns off all the haptics of the app, and
the version of the app.

Forget deletes the pairing key of that machine on this iPhone, and its conversations
leave the app. Nothing changes on the machine: to take the access of the phone away,
also revoke it there (see [Revoke a device](#revoke-a-device)). After you forget the
last machine, the app goes back to the first step of the pairing.

## Push notifications

efrd sends a push to a paired phone when all of these are true:

- Something needs you: an approval, a question of the model, a command in the hidden
  shell that waits for input, the end of a job notice turn or job notices at the wake
  limit. The end of a turn that the phone started also pushes when the turn took
  `notify.after_secs` or longer.
- `presence.push` is on in `config.toml`. It is on by default.
- You are not at the terminal of that conversation: efr counts you away
  (`presence.away_after_minutes`, `efr away`), or your newest input in the
  conversation came from the phone.
- The app is not in front on that phone.
- The phone has push on, and it connected to efrd in the last 30 days.

One question pushes once. A question that waits at the terminal pushes when you turn
away from it. While the app is in front, it shows no banner and plays no sound: the
conversation shows the question.

A push shows the title `efr` and one short line, such as `app: a question waits` or
`app: done in 4m 12s`. `app` is the name of the project of the conversation, or the
name of its directory. A push never holds a prompt, a command, a title of a
conversation or text of the model. A tap opens the conversation.

The push goes from efrd to the Expo push service, then to Apple, then to the phone.
efrd keeps no push key: the push key of the app is at Expo. The servers of Expo and of
Apple get the title, the place name and two ids: the id of the machine and the id of
the conversation, which a tap needs. Your prompts, your output and your approvals stay
inside your tailnet. While the Expo push service is down, no push comes.

`efr device list` shows `push on` after the name of each phone that has push on. To
check push, send a test notice to each such phone:

```sh
efr push test
```

After about 14 seconds, efr shows one line for each phone: its name, then the result.

| Result | What it means |
|---|---|
| `sent` | Apple took the notice. |
| `sent to Expo, no receipt yet` | Expo took the notice, and Apple did not answer yet. |
| `not sent: DeviceNotRegistered. Push is off for this phone now. Open the app.` | The token of the phone is no longer good. efrd deleted it. Open the app to send a new one. |
| `not sent: InvalidCredentials. Expo has no valid push key for this app. Run eas credentials.` | The Expo project of the app has no push key. |
| `not sent: UNAUTHORIZED. The Expo account asks for an access token. efr sends none.` | The Expo account has "enhanced push security" on. efr does not support it. |
| `not sent: <code>. Try again later.` | Expo has too many requests now (`MessageRateExceeded`, `TOO_MANY_REQUESTS`). |
| `not sent: <code>.` | Expo refused the notice with another code. |
| `not sent: no answer from exp.host.` | efrd got no answer from the Expo push service. |
| `no paired phone has push on` | No phone sent its token. |

A revoke deletes the token of the phone at once, so the phone gets no push after
`efr device revoke`.

## What the phone cannot do

A phone has fewer rights than your terminal. efrd checks each of these rules itself,
so a changed app cannot get past them:

- A turn from the phone runs with at most `auto`, never `full`, and in `auto` only when
  the prompt asks for `auto`. A prompt that names no mode runs as `cautious`, where efr
  asks for everything outside `$SCRATCH`. See [Permission modes](modes.md). The app
  sends `auto` only while its Face ID unlock is on; efrd cannot see the unlock.
- The phone can answer Leave to the quarantine question, but not Keep.
- Only your own computer answers some questions: a command that runs as another user
  (`sudo`, `doas`, `su`, `pkexec`, `run0`), a background job, and a delete that full
  mode guards. The strip shows "answer on" and the name of the machine in place of the
  buttons, and the sheet tells the reason. The turn waits for your answer there.
- The phone cannot choose "always allow". An always-allow rule changes your config.
  When a question offers it, the app shows Allow and Deny, and tells that the rule is
  set in the terminal.
- The phone cannot type an input for a command, such as a password. The app shows
  that the command can ask for input.
- The phone cannot undo a turn and cannot change settings.
- The phone never gets the `admin` scope, so it cannot pair or revoke a device.

Limits of this version:

- iPhone only. No Android, no iPad layout.
- No attachments, such as images.
- No terminal view of the hidden shell.
- A dark theme only.
- The connection is `ws://` inside Tailscale. WireGuard encrypts it, and efrd adds no
  TLS of its own.

## Revoke a device

List the paired devices:

```sh
efr device list
```

Each row shows the start of the device id, its name, `push on` when the phone has push
on, its Tailscale node, when it was paired and when it was last seen. The list also
shows the revoked devices.
`--json` prints the answer of efrd, for scripts.

Revoke a device when you lose the phone, or when you do not use it any more:

```sh
efr device revoke 0193f2a1
```

The id can be the start of an id, as the list shows it: at least 4 characters, and
only one device may have that start. The open connections of the device close at once. A revoke cannot be undone.
To use the phone again, pair it again with `efr device add`.

## How efrd keeps the listener safe

- efrd listens only on the Tailscale IPv4 address, never on all interfaces.
- For each connection, efrd asks tailscaled who the peer is before it reads a byte.
  When tailscaled cannot tell, efrd closes the connection.
- efrd refuses a connection from this computer itself. So a local process, such as a
  command of the agent, cannot use the listener.
- efrd refuses a connection from an address that is not a Tailscale address, such as a
  device on your home network behind a subnet route.
- efrd refuses tagged devices, shared devices, and devices of other users or other
  tailnets. A paired device must connect from the Tailscale node, the user and the
  tailnet that it paired from.
- efrd refuses a web page. A request with an `Origin` header fails, unless the header
  is the address of the listener: `http://`, the Tailscale IPv4 address of this
  machine, and the port. So a browser cannot reach efrd through a page.
- The phone proves its device key on each connection.
- A revoke closes the device's connections at once, and efrd drops what it had not
  sent yet.
- efrd keeps at most 16 phone connections at one time.

If you install Tailscale again on the phone, the phone becomes a new Tailscale node.
efrd then refuses it, and the app shows "Pair this phone again". Pair it again with
`efr device add`, and revoke the old entry.

[Troubleshooting](../help/troubleshooting.md#the-phone) tells what to do when the
phone does not connect.
