// Scripted subscribe streams, as efrd sends them, for the replay tests and the demo
// session of dev builds. A script writes the wire JSON of each item with the time it
// arrives:
//
// - drafts of a message's text every 16 ms (`conversation.draft_interval_ms`), with the
//   UTF-8 byte offset of each delta;
// - an `assistant_message_updated` every 200 ms with the text since the last one, then
//   `assistant_message_completed` with the whole text;
// - reasoning drafts with a title, tool input drafts, tool output updates, an approval
//   question, and the end of the turn with how full the context is.
//
// The network delivers drafts in bursts; `burst` holds a run of drafts back and lets
// them arrive together. The scenarios cover the cases of R4c.7 of the research: a long
// reply, code blocks, a table, emoji and CJK text, reasoning, tool output and an
// approval in the middle of a turn.

import { utf8Length } from "../core/utf8";

/** One item of a stream with the time it arrives, in ms from the start. */
export interface TimedItem {
  readonly at: number;
  readonly item: unknown;
}

/** A scripted stream and the truth that a client must end with. */
export interface Scenario {
  readonly name: string;
  readonly conversationId: string;
  readonly items: readonly TimedItem[];
  /** The completed text of each message, by `turnId:index`. */
  readonly texts: ReadonlyMap<string, string>;
}

/** When the scripts begin, as wall clock time. */
export const SCRIPT_EPOCH = Date.UTC(2026, 9, 26, 12, 0, 0);

const DRAFT_INTERVAL = 16;
const UPDATE_INTERVAL = 200;

/** Writes one stream. */
export class Script {
  readonly conversationId: string;
  private seq = 0;
  private now = 0;
  private readonly out: TimedItem[] = [];
  private readonly texts = new Map<string, string>();
  private readonly reasoningBytes = new Map<string, number>();

  constructor(conversationId: string) {
    this.conversationId = conversationId;
  }

  /** The time of the next item. */
  get time(): number {
    return this.now;
  }

  /** Moves the clock on by `ms`. */
  wait(ms: number): this {
    this.now += ms;
    return this;
  }

  /** One event. */
  event(kind: string, body: Readonly<Record<string, unknown>>): this {
    this.seq += 1;
    this.push({
      kind: "event",
      seq: this.seq,
      conversation_id: this.conversationId,
      at: new Date(SCRIPT_EPOCH + this.now).toISOString(),
      event: { kind, ...body },
    });
    return this;
  }

  /** One draft of the turn. */
  draft(turnId: string, part: Readonly<Record<string, unknown>>): this {
    this.push({ kind: "draft", turn_id: turnId, after_seq: this.seq, draft: part });
    return this;
  }

  /** A prompt and the start of its turn. */
  prompt(turnId: string, text: string, origin = "shell"): this {
    this.event("prompt_queued", { turn_id: turnId, command_id: `c-${turnId}`, text, origin });
    return this.event("turn_started", { turn_id: turnId, cwd: "/home/me/p/efr" });
  }

  /** Reasoning drafts of `text`, in pieces of about `piece` characters. */
  reason(turnId: string, text: string, title?: string, piece = 24): this {
    let offset = this.reasoningBytes.get(turnId) ?? 0;
    for (const delta of pieces(text, piece)) {
      this.wait(DRAFT_INTERVAL);
      this.draft(turnId, {
        kind: "reasoning",
        offset,
        delta,
        ...(title === undefined ? {} : { title }),
      });
      offset += utf8Length(delta);
    }
    this.reasoningBytes.set(turnId, offset);
    return this;
  }

  /**
   * Streams message `index` of the turn: drafts every 16 ms, an update every 200 ms, then
   * the completed text. Each `burst` drafts arrive at once.
   */
  say(turnId: string, index: number, text: string, options: SayOptions = {}): this {
    const piece = options.piece ?? 12;
    const burst = Math.max(1, options.burst ?? 1);
    let offset = 0;
    let persisted = 0;
    let persistedText = "";
    let sinceUpdate = 0;
    let held = 0;
    let sent = "";
    for (const delta of pieces(text, piece)) {
      held += 1;
      if (held >= burst) {
        this.wait(DRAFT_INTERVAL * burst);
        held = 0;
      }
      this.draft(turnId, { kind: "text", index, offset, delta });
      offset += utf8Length(delta);
      sent += delta;
      sinceUpdate += DRAFT_INTERVAL;
      if (sinceUpdate >= UPDATE_INTERVAL) {
        sinceUpdate = 0;
        const update = sent.slice(persistedText.length);
        this.event("assistant_message_updated", {
          turn_id: turnId,
          index,
          offset: persisted,
          delta: update,
        });
        persisted += utf8Length(update);
        persistedText = sent;
      }
    }
    this.wait(DRAFT_INTERVAL);
    this.event("assistant_message_completed", { turn_id: turnId, index, text });
    this.texts.set(`${turnId}:${index}`, text);
    return this;
  }

  /** A tool call: its input drafts, start, output updates and end. */
  tool(turnId: string, callId: string, tool: string, input: unknown, output: string): this {
    const inputBytes = utf8Length(JSON.stringify(input));
    for (let bytes = 64; bytes < inputBytes; bytes += 64) {
      this.wait(DRAFT_INTERVAL);
      this.draft(turnId, { kind: "tool_input", call: 0, tool, bytes });
    }
    this.wait(DRAFT_INTERVAL);
    this.event("tool_call_started", { turn_id: turnId, call_id: callId, tool, input });
    return this.toolOutput(turnId, callId, output);
  }

  /** The output and the end of a started tool call. */
  toolOutput(turnId: string, callId: string, output: string, isError = false): this {
    let tail = "";
    for (const line of output.split("\n")) {
      this.wait(UPDATE_INTERVAL);
      tail = `${tail}${line}\n`.slice(-4096);
      this.event("tool_call_output_updated", {
        turn_id: turnId,
        call_id: callId,
        tail,
        bytes: utf8Length(tail),
      });
    }
    this.wait(DRAFT_INTERVAL);
    return this.event("tool_call_completed", {
      turn_id: turnId,
      call_id: callId,
      output,
      truncated: false,
      is_error: isError,
      ...(isError ? { exit_code: 1 } : { exit_code: 0 }),
    });
  }

  /** The end of the turn with how full the context is. */
  done(turnId: string, tokens = 89_400): this {
    this.draft(turnId, { kind: "context", tokens, limit: 206_720, window: 272_000 });
    this.wait(DRAFT_INTERVAL);
    return this.event("turn_completed", {
      turn_id: turnId,
      context: { tokens, limit: 206_720, window: 272_000 },
    });
  }

  /** The stream as written. */
  build(name: string): Scenario {
    return {
      name,
      conversationId: this.conversationId,
      items: [...this.out],
      texts: new Map(this.texts),
    };
  }

  private push(item: unknown): void {
    this.out.push({ at: this.now, item });
  }
}

/** The options of `Script.say`. */
export interface SayOptions {
  /** About how many characters each draft carries. */
  readonly piece?: number;
  /** How many drafts arrive together. */
  readonly burst?: number;
}

/** `text` in pieces of about `size` code points, cut at code point boundaries. */
function pieces(text: string, size: number): string[] {
  const points = Array.from(text);
  const out: string[] = [];
  for (let i = 0; i < points.length; i += size) {
    out.push(points.slice(i, i + size).join(""));
  }
  return out;
}

const CONVERSATION = "019a9b1c-3d00-7a10-8b20-0000000000c1";

const turn = (n: number) => `019a9b1c-3d00-7a10-8b20-0000000001${n.toString().padStart(2, "0")}`;

const call = (n: number) => `019a9b1c-3d00-7a10-8b20-0000000002${n.toString().padStart(2, "0")}`;

const CODE_REPLY = [
  "Here is the fix. The loop read past the end of the buffer:",
  "",
  "```rust",
  "fn sum(values: &[u32]) -> u64 {",
  "    let mut total = 0u64;",
  "",
  "    for value in values {",
  "        total += u64::from(*value);",
  "    }",
  "    total",
  "}",
  "```",
  "",
  "And the test:",
  "",
  "```bash",
  "cargo nextest run -p efr-store sum",
  "```",
  "",
  "The change in the patch:",
  "",
  "```diff",
  "--- a/src/sum.rs",
  "+++ b/src/sum.rs",
  "@@ -1,4 +1,4 @@",
  " fn sum(values: &[u32]) -> u64 {",
  "-    for i in 0..=values.len() {",
  "+    for value in values {",
  "         total += u64::from(*value);",
  "     }",
  "```",
  "",
  "Run it again to check.",
].join("\n");

const TABLE_REPLY = [
  "The disk use by folder:",
  "",
  "| Folder | Size | Share |",
  "|---|---:|---:|",
  "| /var/log/journal | 3.1 GiB | 62% |",
  "| /var/cache/pacman | 1.2 GiB | 24% |",
  "| /tmp | 0.7 GiB | 14% |",
  "",
  "The journal is the **largest** part. You can set `SystemMaxUse=500M`.",
].join("\n");

const MULTIBYTE_REPLY = [
  "The file is `größe.log` 🙂 in ~/日志/, café ñ.",
  "",
  "- 日本語のテキストは単語の間にスペースがありません。",
  "- 한국어 문장도 있습니다.",
  "- Emoji with joiners: 👩‍💻 🏳️‍🌈 👍🏽",
  "",
  "Done ✅.",
].join("\n");

function longReply(): string {
  const parts: string[] = ["# A long answer", ""];
  for (let i = 1; i <= 60; i++) {
    parts.push(
      `${i}. Step ${i} reads the config, checks the **value** of \`key_${i}\`, and writes a line to the log. It does not change the file when the value is the same.`,
    );
    if (i % 10 === 0) {
      parts.push("", `## Part ${i / 10}`, "", "> A note: each part ends with a quote.", "");
    }
  }
  return parts.join("\n");
}

/** Every scenario of R4c.7, for the tests and the demo. */
export function scenarios(): Scenario[] {
  return [
    new Script(CONVERSATION)
      .prompt(turn(1), "why is the disk full")
      .say(turn(1), 0, longReply(), { piece: 18, burst: 3 })
      .done(turn(1))
      .build("long reply"),
    new Script(CONVERSATION)
      .prompt(turn(2), "fix the sum function")
      .say(turn(2), 0, CODE_REPLY, { piece: 7 })
      .done(turn(2))
      .build("code blocks"),
    new Script(CONVERSATION)
      .prompt(turn(3), "what uses the disk")
      .say(turn(3), 0, TABLE_REPLY, { piece: 5, burst: 4 })
      .done(turn(3))
      .build("table"),
    new Script(CONVERSATION)
      .prompt(turn(4), "name the file")
      .say(turn(4), 0, MULTIBYTE_REPLY, { piece: 3 })
      .done(turn(4))
      .build("emoji and cjk"),
    new Script(CONVERSATION)
      .prompt(turn(5), "think first")
      .reason(
        turn(5),
        "**Checking the logs**\n\nThe journal holds most of it.",
        "Checking the logs",
      )
      .say(turn(5), 0, "The journal holds most of the space.")
      .done(turn(5))
      .build("reasoning"),
    toolAndApproval(),
  ];
}

/** A turn with text, a tool call, an approval in the middle, and more text. */
export function toolAndApproval(decision: "allow" | "deny" = "allow"): Scenario {
  const t = turn(6);
  const script = askApproval(new Script(CONVERSATION), t).wait(1500);
  return finishApproval(script, t, decision, "phone").build("tool and approval");
}

/** The id of the call that waits for the question of `askApproval`. */
export const APPROVAL_CALL = call(2);

/** Writes a turn up to its approval question: text, a tool call, more text, the question. */
export function askApproval(script: Script, t: string): Script {
  return script
    .prompt(t, "clean the journal")
    .say(t, 0, "I will look at the journal first.")
    .tool(
      t,
      call(1),
      "shell",
      { command: "journalctl --disk-usage" },
      "Archived and active journals take up 3.1G in the file system.",
    )
    .say(t, 1, "It takes 3.1 GB. I ask before I clean it.")
    .wait(16)
    .event("tool_call_started", {
      turn_id: t,
      call_id: APPROVAL_CALL,
      tool: "shell",
      input: { command: "sudo journalctl --vacuum-size=500M" },
    })
    .event("approval_requested", {
      turn_id: t,
      call_id: APPROVAL_CALL,
      summary: "run `journalctl --vacuum-size=500M` as root",
      diff_preview:
        "--- a/etc/systemd/journald.conf\n+++ b/etc/systemd/journald.conf\n@@ -1 +1 @@\n-#SystemMaxUse=\n+SystemMaxUse=500M\n",
      interactive: true,
      expires_in_secs: 120,
    });
}

/** Writes the rest of the turn of `askApproval` after the answer `decision`. */
export function finishApproval(
  script: Script,
  t: string,
  decision: "allow" | "deny",
  origin: string,
): Script {
  script.event("approval_resolved", { turn_id: t, call_id: APPROVAL_CALL, decision, origin });
  if (decision === "allow") {
    script.toolOutput(t, APPROVAL_CALL, "Vacuuming done, freed 2.6G of archived journals.");
  } else {
    script.wait(16).event("tool_call_completed", {
      turn_id: t,
      call_id: APPROVAL_CALL,
      output: "The user denied the call.",
      truncated: false,
      is_error: true,
    });
  }
  return script
    .say(t, 2, decision === "allow" ? "Done. The journal now takes 500 MB." : "I did not clean it.")
    .done(t, 101_200);
}

/** The conversation of the demo. */
export const DEMO_CONVERSATION = "019a9b1c-3d00-7a10-8b20-0000000000d1";

/** The turn of the demo that asks a question. */
export const DEMO_APPROVAL_TURN = turn(16);

/**
 * The script of the demo: the scenarios as one turn after the other in one conversation,
 * up to the question of the last turn, which waits for the user's answer.
 */
export function demoScript(): Script {
  const script = new Script(DEMO_CONVERSATION)
    .prompt(turn(11), "think first, then name the file")
    .reason(turn(11), "**Checking the logs**\n\nThe journal holds most of it.", "Checking the logs")
    .say(turn(11), 0, MULTIBYTE_REPLY, { piece: 3 })
    .done(turn(11), 41_000)
    .wait(1200)
    .prompt(turn(12), "what uses the disk")
    .say(turn(12), 0, TABLE_REPLY, { piece: 5, burst: 4 })
    .done(turn(12), 52_000)
    .wait(1200)
    .prompt(turn(13), "fix the sum function")
    .say(turn(13), 0, CODE_REPLY, { piece: 7 })
    .done(turn(13), 74_000)
    .wait(1200)
    .prompt(turn(14), "explain each step")
    .say(turn(14), 0, longReply(), { piece: 18, burst: 3 })
    .done(turn(14), 98_000)
    .wait(1200);
  return askApproval(script, DEMO_APPROVAL_TURN);
}
