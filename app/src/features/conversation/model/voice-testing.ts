// Helpers for the tests of voice input: efr-speech that a test drives. Only test files
// import this module.

import type { EventSubscription } from "expo";

import type { EfrSpeechEvents, EfrSpeechNative } from "../../../../modules/efr-speech";

type Listeners = { [E in keyof EfrSpeechEvents]: EfrSpeechEvents[E][] };

/** efr-speech that the test drives: it emits the events and records the calls. */
export class FakeSpeech implements EfrSpeechNative {
  available = true;
  readonly calls: string[] = [];
  /** How `start` ends: at once, or with an error that has this `code`. */
  startError: string | undefined;
  private readonly listeners: Listeners = { phase: [], transcript: [], end: [] };

  isAvailable(): Promise<boolean> {
    return Promise.resolve(this.available);
  }

  start(): Promise<void> {
    this.calls.push("start");
    const code = this.startError;
    return code === undefined
      ? Promise.resolve()
      : Promise.reject(Object.assign(new Error(code), { code }));
  }

  stop(): Promise<void> {
    this.calls.push("stop");
    return Promise.resolve();
  }

  addListener<E extends keyof EfrSpeechEvents>(
    event: E,
    listener: EfrSpeechEvents[E],
  ): EventSubscription {
    const list: EfrSpeechEvents[E][] = this.listeners[event];
    list.push(listener);
    return {
      remove: () => {
        list.splice(list.indexOf(listener), 1);
      },
    };
  }

  phase(phase: "model" | "recording"): void {
    for (const listener of this.listeners.phase) {
      listener({ phase });
    }
  }

  transcript(text: string): void {
    for (const listener of this.listeners.transcript) {
      listener({ text });
    }
  }

  end(reason: "stopped" | "interrupted" | "failed"): void {
    for (const listener of this.listeners.end) {
      listener({ reason, message: reason === "failed" ? "the analyzer failed" : null });
    }
  }

  /** The listeners that still listen. */
  get listening(): number {
    return (
      this.listeners.phase.length + this.listeners.transcript.length + this.listeners.end.length
    );
  }
}
