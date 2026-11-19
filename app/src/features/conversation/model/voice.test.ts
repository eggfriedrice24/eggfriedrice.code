import { describe, expect, it } from "vitest";

import { FakeClock, flush } from "@/lib/net/testing";
import { MemoryKeyValue } from "@/lib/store/testing";
import { createDrafts, draftKey } from "./drafts";
import { CONVERSATION_A, CONVERSATION_B, DESKTOP } from "./outbox-testing";
import {
  MAX_RECORDING_MS,
  MICROPHONE_DENIED,
  createVoice,
  stopLabel,
  trailingWhileVoice,
  voiceOf,
  type Voice,
} from "./voice";
import { FakeSpeech } from "./voice-testing";

const KEY = draftKey(DESKTOP, CONVERSATION_A);

/** The wall clock of the tests, in ms since the epoch. */
const NOW = Date.UTC(2026, 10, 18, 14, 0);

async function setup(text = ""): Promise<{
  readonly voice: Voice;
  readonly speech: FakeSpeech;
  readonly clock: FakeClock;
  readonly drafts: ReturnType<typeof createDrafts>;
  readonly haptics: { count: number };
  readonly errors: unknown[];
}> {
  const speech = new FakeSpeech();
  const clock = new FakeClock();
  const drafts = createDrafts({ storage: new MemoryKeyValue() });
  drafts.set(KEY, text);
  const haptics = { count: 0 };
  const errors: unknown[] = [];
  const voice = createVoice({
    speech,
    drafts,
    clock,
    now: () => NOW,
    haptic: () => {
      haptics.count += 1;
    },
    onError: (error) => errors.push(error),
  });
  await flush();
  return { voice, speech, clock, drafts, haptics, errors };
}

describe("the voice controller", () => {
  it("shows no microphone where iOS has no speech support", async () => {
    const speech = new FakeSpeech();
    speech.available = false;
    const voice = createVoice({
      speech,
      drafts: createDrafts({ storage: new MemoryKeyValue() }),
      clock: new FakeClock(),
      now: () => NOW,
      haptic: () => undefined,
      onError: () => undefined,
    });
    await flush();
    expect(voice.store.get()).toEqual({ kind: "off" });
    voice.start(KEY, 0);
    expect(speech.calls).toEqual([]);
  });

  it("puts the words at the cursor, after the text that was there, and sends nothing", async () => {
    const { voice, speech, drafts, haptics } = await setup("Look at the header. ");
    voice.start(KEY, "Look at the header.".length);
    expect(voice.store.get()).toEqual({ kind: "preparing", key: KEY, model: false });
    speech.phase("recording");
    expect(voice.store.get()).toEqual({ kind: "recording", key: KEY, startedAt: NOW });
    expect(haptics.count).toBe(1);
    speech.transcript("It is cut");
    expect(drafts.text(KEY)).toBe("Look at the header. It is cut ");
    speech.transcript("It is cut off on small phones.");
    expect(drafts.text(KEY)).toBe("Look at the header. It is cut off on small phones. ");
    voice.stop();
    expect(voice.store.get()).toEqual({ kind: "transcribing", key: KEY });
    expect(haptics.count).toBe(2);
    speech.end("stopped");
    expect(voice.store.get()).toEqual({ kind: "idle", problem: undefined });
    expect(speech.calls).toEqual(["start", "stop"]);
  });

  it("says that iOS gets the speech model the first time for a language", async () => {
    const { voice, speech } = await setup();
    voice.start(KEY, 0);
    speech.phase("model");
    expect(voice.store.get()).toEqual({ kind: "preparing", key: KEY, model: true });
  });

  it("puts only the new words at the end when the user changed the words of the recording", async () => {
    const { voice, speech, drafts } = await setup("Hi");
    voice.start(KEY, 2);
    speech.phase("recording");
    speech.transcript("there");
    // Text that the user types after the words stays after them.
    drafts.set(KEY, "Hi there, you");
    speech.transcript("there friend");
    expect(drafts.text(KEY)).toBe("Hi there friend, you");
    drafts.set(KEY, "Hi THERE, you");
    speech.transcript("there friend and more");
    expect(drafts.text(KEY)).toBe("Hi THERE, you and more");
    speech.transcript("there friend and more words");
    expect(drafts.text(KEY)).toBe("Hi THERE, you and more words");
  });

  it("puts a space after the words when a word follows the cursor", async () => {
    const { voice, speech, drafts } = await setup("Hello world");
    voice.start(KEY, 6);
    speech.phase("recording");
    speech.transcript("there");
    expect(drafts.text(KEY)).toBe("Hello there world");
    speech.transcript("there big");
    expect(drafts.text(KEY)).toBe("Hello there big world");
  });

  it("stops at the limit of 5 minutes and keeps the text", async () => {
    const { voice, speech, clock, drafts } = await setup();
    voice.start(KEY, 0);
    speech.phase("recording");
    speech.transcript("A long talk");
    await clock.advance(MAX_RECORDING_MS);
    expect(speech.calls).toEqual(["start", "stop"]);
    expect(voice.store.get().kind).toBe("transcribing");
    speech.end("stopped");
    expect(drafts.text(KEY)).toBe("A long talk");
  });

  it("names an interruption, a silent recording, a denied microphone and an error, until the next tap", async () => {
    const { voice, speech, errors } = await setup();
    voice.start(KEY, 0);
    speech.phase("recording");
    speech.transcript("so far");
    speech.end("interrupted");
    expect(voice.store.get()).toEqual({ kind: "idle", problem: "interrupted" });

    voice.start(KEY, 0);
    speech.phase("recording");
    voice.stop();
    speech.end("stopped");
    expect(voice.store.get()).toEqual({ kind: "idle", problem: "silent" });

    speech.startError = MICROPHONE_DENIED;
    voice.start(KEY, 0);
    await flush();
    expect(voice.store.get()).toEqual({ kind: "idle", problem: "denied" });
    expect(errors).toEqual([]);

    speech.startError = undefined;
    voice.start(KEY, 0);
    speech.phase("recording");
    speech.end("failed");
    expect(voice.store.get()).toEqual({ kind: "idle", problem: "failed" });
    expect(errors).toHaveLength(1);
    voice.dismiss();
    expect(voice.store.get()).toEqual({ kind: "idle", problem: undefined });
  });

  it("stops the module when the user stopped before the microphone ran, with no line", async () => {
    const { voice, speech, haptics } = await setup();
    voice.start(KEY, 0);
    voice.stop();
    speech.phase("recording");
    expect(voice.store.get().kind).toBe("transcribing");
    expect(speech.calls).toEqual(["start", "stop", "stop"]);
    speech.end("stopped");
    expect(voice.store.get()).toEqual({ kind: "idle", problem: undefined });
    expect(haptics.count).toBe(0);
  });

  it("shows the recording only in the composer of its draft", async () => {
    const { voice, speech } = await setup();
    voice.start(KEY, 0);
    speech.phase("recording");
    const other = draftKey(DESKTOP, CONVERSATION_B);
    expect(voiceOf(voice.store.get(), other)).toEqual({ kind: "idle", problem: undefined });
    expect(voiceOf(voice.store.get(), KEY).kind).toBe("recording");
  });

  it("stops its listeners", async () => {
    const { voice, speech } = await setup();
    voice.dispose();
    expect(speech.listening).toBe(0);
  });

  it("keeps Send and the capsule off while a recording writes into the draft", () => {
    const recording = { kind: "recording", key: KEY, startedAt: 0 } as const;
    expect(trailingWhileVoice({ kind: "send", enabled: true }, recording)).toEqual({
      kind: "send",
      enabled: false,
    });
    expect(trailingWhileVoice({ kind: "stop", capsule: "steer" }, recording)).toEqual({
      kind: "stop",
      capsule: null,
    });
    const send = { kind: "send", enabled: true } as const;
    expect(trailingWhileVoice(send, { kind: "idle", problem: undefined })).toBe(send);
  });

  it("reads the time of a recording to VoiceOver", () => {
    expect(stopLabel(1)).toBe("Stop recording, 1 second");
    expect(stopLabel(12)).toBe("Stop recording, 12 seconds");
  });
});
