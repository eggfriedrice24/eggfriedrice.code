// The demo server of the dev builds (plan section 4, W4-DEMO-MEASURE): a small efrd for
// the demo machines (demo-machines.ts) behind the `Socket` interface of the app. The
// runtime of the app talks to it as to a real efrd, so every screen runs its real code:
// the pairing flow, hello and the proof, the subscriptions, the queries and the
// mutations.
//
// - It speaks the frames of docs/protocol.md "Remote devices": `hello` (a second hello
//   gets `conflict`, as the liveness probe needs), `device.enroll` after a short wait (the
//   user says yes at the terminal), `device.prove`, `conversation.subscribe` (a snapshot,
//   or the events after `after_seq`, then the drafts of the running turn and the live
//   items), the conversation lists, `prompt.send` (idempotent by `command_id`),
//   `approval.respond`, `turn.steer`, `turn.interrupt` and `prompt.withdraw`.
// - A stage (`stage`) sets the conversations of every demo machine for one board of the
//   gallery, and the mode of each machine: online, offline, silent (the socket opens and
//   efrd never answers), starting, re-pair needed, or too old.
// - The conversations play the scripts of demo-conversations.ts on the clock.
//
// It reaches no network. The clock comes in as a parameter, so the tests run it on a
// fake clock. Pure.

import type { Clock, Timer } from "@/lib/net/clock";
import {
  CLOSE_ABNORMAL,
  type Socket,
  type SocketFactory,
  type SocketListener,
} from "@/lib/net/socket";
import { isJsonObject } from "@/lib/protocol/envelope";
import {
  PROTOCOL_VERSION,
  type ErrorBody,
  type HelloResult,
  type RecentConversation,
  type ScopeName,
} from "@/lib/protocol/generated";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import {
  demoConversation,
  type DemoConversationKind,
  type DemoConversationScript,
} from "./demo-conversations";
import { DemoLog, type WireItem } from "./demo-log";
import {
  DEMO_MACHINES,
  demoMachine,
  demoMachineAt,
  type DemoMachine,
  type DemoMachineKey,
} from "./demo-machines";
import { DEMO_CWD, DemoScript, type DemoStep } from "./demo-script";

/** How a demo machine answers. */
export type MachineMode =
  /** efrd answers everything. */
  | "online"
  /** The socket fails at once, as when Tailscale cannot reach the machine. */
  | "offline"
  /** The socket opens, but efrd never answers: the phone waits, then tries again. */
  | "silent"
  /** efrd answers hello with `busy`: it is starting. */
  | "starting"
  /** efrd refuses the proof and asks for a new pairing (`re_pair`). */
  | "repair"
  /** efrd speaks an older protocol. */
  | "old";

/** One conversation of a stage. */
export interface StageConversation {
  /** The name of the conversation in the stage, such as `streaming`. */
  readonly key: string;
  readonly machine: DemoMachineKey;
  readonly kind: DemoConversationKind;
  /** How long ago its history ended. */
  readonly minutesAgo: number;
  /** The snapshot of a subscription comes this late, for the loading line. */
  readonly snapshotDelayMs?: number;
}

/** A change of a machine's mode after the first subscription to a conversation. */
export interface StageChange {
  /** The key of the conversation whose first subscription starts the wait. */
  readonly after: string;
  readonly afterMs: number;
  readonly machine: DemoMachineKey;
  readonly mode: MachineMode;
}

/** What the demo machines hold for one board. */
export interface DemoStage {
  readonly name: string;
  /** The mode of each machine; a machine that is not named is online. */
  readonly modes?: Readonly<Partial<Record<DemoMachineKey, MachineMode>>>;
  readonly conversations: readonly StageConversation[];
  readonly changes?: readonly StageChange[];
}

/** What the server needs. */
export interface DemoServerDeps {
  /** The clock of the scripts. Its time is also the time of the events. */
  readonly clock: Clock;
  /** How long the user takes to say yes at the terminal when the phone pairs. */
  readonly confirmMs?: number;
}

/** The version of efrd that the demo machines give. */
export const DEMO_VERSION = "0.0.5";

/** The scopes of a demo device. */
const SCOPES: ScopeName[] = ["read", "operate", "approve"];

/** How long the user takes to say yes at the terminal, by default. */
export const DEFAULT_CONFIRM_MS = 1200;

/** The wait that a starting efrd asks for. */
const STARTING_RETRY_MS = 3000;

/** The reply of the demo machine to a prompt from the phone. */
const DEMO_REPLY =
  "This is a demo machine of the dev build. It plays a scripted answer and reaches no computer. To use efr from this phone, run `efr device add` on your computer and scan the code.";

const ID_PREFIX = "0199d3e0-de31-7000-8000-";

type Json = Readonly<Record<string, unknown>>;

/** A subscription of one session. */
interface Subscriber {
  readonly session: DemoSession;
  readonly id: number;
}

/** A demo efrd for all demo machines. */
export class DemoServer {
  private readonly deps: DemoServerDeps;
  private readonly modes = new Map<DemoMachineKey, MachineMode>();
  private readonly sessions = new Set<DemoSession>();
  private conversationsById = new Map<string, ServedConversation>();
  private changeTimers: Timer[] = [];
  private nextId = 1;
  private stageName = "";

  constructor(deps: DemoServerDeps) {
    this.deps = deps;
  }

  /** The clock of the server. */
  get clock(): Clock {
    return this.deps.clock;
  }

  /** How long the user takes to say yes at the terminal. */
  get confirmMs(): number {
    return this.deps.confirmMs ?? DEFAULT_CONFIRM_MS;
  }

  /** The name of the stage now. */
  get stageNow(): string {
    return this.stageName;
  }

  /** A fresh id, never given before by this server. */
  newId(): string {
    const id = `${ID_PREFIX}${this.nextId.toString(16).padStart(12, "0")}`;
    this.nextId += 1;
    return id;
  }

  /** The mode of `machine`. */
  mode(machine: DemoMachineKey): MachineMode {
    return this.modes.get(machine) ?? "online";
  }

  /**
   * Sets the mode of `machine`. A machine that stops answering drops its sockets, so the
   * phone connects again and meets the new mode.
   */
  setMode(machine: DemoMachineKey, mode: MachineMode): void {
    this.modes.set(machine, mode);
    if (mode === "online") {
      return;
    }
    for (const session of this.sessions) {
      if (session.machine.key === machine) {
        session.drop();
      }
    }
  }

  /**
   * Sets `stage`: new conversations on every machine and the modes of the machines.
   * The conversations of the stage before are gone. It gives the ref of each
   * conversation of the stage by its key.
   */
  stage(stage: DemoStage): ReadonlyMap<string, ConversationRef> {
    for (const conversation of this.conversationsById.values()) {
      conversation.stop();
    }
    for (const timer of this.changeTimers.splice(0)) {
      timer.cancel();
    }
    this.stageName = stage.name;
    this.conversationsById = new Map();
    const refs = new Map<string, ConversationRef>();
    const now = this.clock.now();
    for (const spec of stage.conversations) {
      const machine = demoMachine(spec.machine);
      const script = demoConversation(spec.kind, () => this.newId());
      const conversation = new ServedConversation(this, machine, spec, script, now);
      this.conversationsById.set(conversation.id, conversation);
      refs.set(spec.key, { machine: machine.daemonId, id: conversation.id });
    }
    for (const changes of stage.changes ?? []) {
      const ref = refs.get(changes.after);
      const conversation = ref === undefined ? undefined : this.conversationsById.get(ref.id);
      conversation?.onFirstSubscribe(() => {
        this.changeTimers.push(
          this.clock.setTimer(changes.afterMs, () => this.setMode(changes.machine, changes.mode)),
        );
      });
    }
    for (const machine of DEMO_MACHINES) {
      this.setMode(machine.key, stage.modes?.[machine.key] ?? "online");
    }
    return refs;
  }

  /** The conversations of `machine` now, newest first. */
  conversations(machine: DemoMachineKey): ServedConversation[] {
    return [...this.conversationsById.values()]
      .filter((conversation) => conversation.machine.key === machine)
      .sort((a, b) => b.updatedAt - a.updatedAt);
  }

  /** The conversation `id`, if the stage has it. */
  conversation(id: string): ServedConversation | undefined {
    return this.conversationsById.get(id);
  }

  /** Adds a conversation that a prompt of the phone starts on `machine`. */
  startConversation(machine: DemoMachine, title: string): ServedConversation {
    const conversation = new ServedConversation(
      this,
      machine,
      { key: title, machine: machine.key, kind: "notes", minutesAgo: 0 },
      { title, history: new DemoScript().created().steps(), live: [] },
      this.clock.now(),
    );
    this.conversationsById.set(conversation.id, conversation);
    return conversation;
  }

  /** Opens a socket to `url`, which must be the address of a demo machine. */
  connect(url: string, listener: SocketListener): Socket {
    const machine = demoMachineAt(url);
    if (machine === undefined) {
      throw new Error(`${url} is not a demo machine`);
    }
    const session = new DemoSession(this, machine, listener);
    this.sessions.add(session);
    session.begin(this.mode(machine.key));
    return session;
  }

  /** A socket factory that sends the demo machines here and every other URL to `base`. */
  sockets(base: SocketFactory): SocketFactory {
    return (url, listener) =>
      demoMachineAt(url) === undefined ? base(url, listener) : this.connect(url, listener);
  }

  /** The number of open sockets to `machine`. */
  openSockets(machine: DemoMachineKey): number {
    return [...this.sessions].filter((session) => session.machine.key === machine && session.open)
      .length;
  }

  /** Forgets a closed session. */
  closed(session: DemoSession): void {
    this.sessions.delete(session);
    for (const conversation of this.conversationsById.values()) {
      conversation.unsubscribeAll(session);
    }
  }

  /** Stops every timer and drops every socket. */
  dispose(): void {
    for (const conversation of this.conversationsById.values()) {
      conversation.stop();
    }
    for (const timer of this.changeTimers.splice(0)) {
      timer.cancel();
    }
    for (const session of this.sessions) {
      session.drop();
    }
  }
}

/** One conversation of a demo machine, with its script and its subscribers. */
export class ServedConversation {
  readonly machine: DemoMachine;
  readonly log: DemoLog;
  private readonly server: DemoServer;
  private queue: DemoStep[];
  private readonly snapshotDelayMs: number | undefined;
  private readonly subscribers = new Set<Subscriber>();
  private readonly firstListeners: (() => void)[] = [];
  private readonly commands = new Map<string, Json>();
  private readonly queuedPrompts = new Map<
    string,
    { readonly seq: number; readonly text: string }
  >();
  private timer: Timer | undefined;
  private hold: Extract<DemoStep, { kind: "hold" }> | undefined;
  private subscribed = false;
  private stopped = false;

  constructor(
    server: DemoServer,
    machine: DemoMachine,
    spec: StageConversation,
    script: DemoConversationScript,
    now: number,
  ) {
    this.server = server;
    this.machine = machine;
    this.snapshotDelayMs = spec.snapshotDelayMs;
    this.log = new DemoLog({
      id: server.newId(),
      title: script.title,
      cwd: DEMO_CWD,
      createdAt: now - spec.minutesAgo * 60_000 - totalWait(script.history) - 60_000,
    });
    this.queue = [...script.live];
    // The history happened before: its times end `minutesAgo` before now.
    let at = now - spec.minutesAgo * 60_000 - totalWait(script.history);
    const history = [...script.history];
    while (history.length > 0) {
      const step = history.shift();
      if (step === undefined) {
        break;
      }
      at += step.wait;
      if (step.kind === "hold") {
        this.hold = step;
        this.queue = [...history, ...this.queue];
        break;
      }
      this.track(this.log.send(step, at));
    }
  }

  get id(): string {
    return this.log.facts.id;
  }

  /** The last time of the conversation, ms since the epoch. */
  get updatedAt(): number {
    return Date.parse(this.log.summary().updated_at);
  }

  /** The conversation as `conversations.recent` gives it. */
  recent(): RecentConversation {
    return { summary: this.log.summary(), route: { kind: "outside" }, turns: this.log.turns };
  }

  /** Calls `listener` at the first subscription. */
  onFirstSubscribe(listener: () => void): void {
    if (this.subscribed) {
      listener();
    } else {
      this.firstListeners.push(listener);
    }
  }

  /** A subscription of `session` with request id `id`. */
  subscribe(session: DemoSession, id: number, afterSeq: number | undefined): void {
    const subscriber: Subscriber = { session, id };
    const deliver = (): void => {
      if (session.open) {
        const items =
          afterSeq === undefined ? [this.log.snapshot()] : this.log.eventsAfter(afterSeq);
        for (const item of [...items, ...this.log.liveDrafts()]) {
          session.item(id, item);
        }
        this.subscribers.add(subscriber);
      }
    };
    if (this.snapshotDelayMs === undefined) {
      deliver();
    } else {
      this.server.clock.setTimer(this.snapshotDelayMs, deliver);
    }
    if (!this.subscribed) {
      this.subscribed = true;
      for (const listener of this.firstListeners.splice(0)) {
        listener();
      }
      this.pump();
    }
  }

  /** Ends the subscription `id` of `session`. */
  unsubscribe(session: DemoSession, id: number): void {
    for (const subscriber of this.subscribers) {
      if (subscriber.session === session && subscriber.id === id) {
        this.subscribers.delete(subscriber);
      }
    }
  }

  /** Ends every subscription of `session`. */
  unsubscribeAll(session: DemoSession): void {
    for (const subscriber of this.subscribers) {
      if (subscriber.session === session) {
        this.subscribers.delete(subscriber);
      }
    }
  }

  /** The earlier result of the write with `commandId`, for an idempotent retry. */
  earlier(commandId: string): Json | undefined {
    return this.commands.get(commandId);
  }

  /** Keeps the result of the write with `commandId`. */
  remember(commandId: string, result: Json): void {
    this.commands.set(commandId, result);
  }

  /** A prompt of the phone: it runs now, or after the turn that runs. */
  prompt(text: string, commandId: string): { seq: number; turnId: string; queued: boolean } {
    const turn = this.server.newId();
    const queued = this.log.runningTurn !== undefined || this.queue.length > 0;
    const sent = this.sendNow({
      kind: "event",
      wait: 0,
      event: { kind: "prompt_queued", turn_id: turn, command_id: commandId, text, origin: "phone" },
    });
    this.queuedPrompts.set(turn, { seq: sent, text });
    const reply = new DemoScript()
      .wait(queued ? 300 : 400)
      .start(turn)
      .reason(turn, "The phone asks. The demo machine answers with a script.", undefined, {
        every: 40,
      })
      .say(turn, 0, DEMO_REPLY, { piece: 5, every: 30 })
      .done(turn, 12_000);
    this.queue.push(...reply.steps());
    this.pump();
    return { seq: sent, turnId: turn, queued };
  }

  /** The phone answers the question `callId`. Gives the seq of the answer, or undefined. */
  answer(callId: string, decision: "allow" | "deny"): number | undefined {
    const hold = this.hold;
    if (hold === undefined || hold.callId !== callId) {
      return undefined;
    }
    this.hold = undefined;
    const seq = this.sendNow({
      kind: "event",
      wait: 0,
      event: {
        kind: "approval_resolved",
        turn_id: hold.turnId,
        call_id: callId,
        decision,
        origin: "phone",
      },
    });
    this.queue = [...(decision === "allow" ? hold.allow : hold.deny), ...this.queue];
    this.pump();
    return seq;
  }

  /** The phone stops the running turn. Gives the seq and the turn, or undefined. */
  interrupt(): { seq: number; turnId: string } | undefined {
    const turn = this.log.runningTurn;
    if (turn === undefined) {
      return undefined;
    }
    const hold = this.hold;
    if (hold !== undefined && hold.turnId === turn) {
      this.hold = undefined;
      this.sendNow({
        kind: "event",
        wait: 0,
        event: { kind: "approval_expired", turn_id: turn, call_id: hold.callId },
      });
    }
    const seq = this.sendNow({
      kind: "event",
      wait: 0,
      event: { kind: "turn_interrupt_requested", turn_id: turn, origin: "phone" },
    });
    this.queue = this.queue.filter((step) => stepTurn(step) !== turn);
    this.queue.unshift({
      kind: "event",
      wait: 300,
      event: { kind: "turn_interrupted", turn_id: turn },
    });
    this.restart();
    return { seq, turnId: turn };
  }

  /** The phone steers the running turn. Gives the seq and the turn, or undefined. */
  steer(text: string): { seq: number; turnId: string } | undefined {
    const turn = this.log.runningTurn;
    if (turn === undefined) {
      return undefined;
    }
    const seq = this.sendNow({
      kind: "event",
      wait: 0,
      event: { kind: "turn_steered", turn_id: turn, text },
    });
    this.queue.unshift({
      kind: "event",
      wait: 1200,
      event: { kind: "steering_delivered", turn_id: turn, steers: [seq] },
    });
    this.restart();
    return { seq, turnId: turn };
  }

  /** The phone takes back the queued prompt of `turnId`. */
  withdraw(turnId: string): { seq: number; text: string } | undefined {
    const prompt = this.queuedPrompts.get(turnId);
    if (prompt === undefined || this.log.runningTurn === turnId) {
      return undefined;
    }
    this.queuedPrompts.delete(turnId);
    this.queue = this.queue.filter((step) => stepTurn(step) !== turnId);
    this.sendNow({
      kind: "event",
      wait: 0,
      event: { kind: "prompt_withdrawn", turn_id: turnId, origin: "phone" },
    });
    return prompt;
  }

  /** Stops the script. */
  stop(): void {
    this.stopped = true;
    this.timer?.cancel();
    this.timer = undefined;
    this.subscribers.clear();
  }

  /** Sends one step now, outside the queue. Gives its seq. */
  private sendNow(step: Extract<DemoStep, { kind: "event" }>): number {
    this.deliver(this.log.send(step, this.server.clock.now()));
    return this.log.lastSeq;
  }

  /** Plans the next step of the queue again, after a change at its front. */
  private restart(): void {
    this.timer?.cancel();
    this.timer = undefined;
    this.pump();
  }

  /** Plays the queue on the clock, up to a hold. */
  private pump(): void {
    if (this.stopped || !this.subscribed || this.timer !== undefined || this.hold !== undefined) {
      return;
    }
    const next = this.queue[0];
    if (next === undefined) {
      return;
    }
    this.timer = this.server.clock.setTimer(next.wait, () => {
      this.timer = undefined;
      const step = this.queue.shift();
      if (step === undefined || this.stopped) {
        return;
      }
      if (step.kind === "hold") {
        this.hold = step;
        return;
      }
      const item = this.log.send(step, this.server.clock.now());
      this.track(item);
      this.deliver(item);
      this.pump();
    });
  }

  /** Follows the prompts that wait in the queue. */
  private track(item: WireItem): void {
    if (item.kind === "event" && item.event.kind === "turn_started") {
      this.queuedPrompts.delete(item.event.turn_id);
    }
  }

  private deliver(item: WireItem): void {
    for (const subscriber of this.subscribers) {
      subscriber.session.item(subscriber.id, item);
    }
  }
}

/** The turn of a step, if it names one. */
function stepTurn(step: DemoStep): string | undefined {
  if (step.kind === "draft" || step.kind === "hold") {
    return step.turnId;
  }
  const event: unknown = step.event;
  if (isJsonObject(event) && typeof event["turn_id"] === "string") {
    return event["turn_id"];
  }
  return undefined;
}

/** The time of a list of steps. */
function totalWait(steps: readonly DemoStep[]): number {
  return steps.reduce((sum, step) => sum + step.wait, 0);
}

/** The hello result of `machine`. */
function helloOf(machine: DemoMachine, protocol: number): HelloResult {
  return {
    daemon_id: machine.daemonId,
    protocol,
    version: protocol === PROTOCOL_VERSION ? DEMO_VERSION : "0.0.3",
    capabilities: {
      drafts_coalesced: true,
      snapshot_live: true,
      device_prove: true,
      approval_local_only: true,
    },
    paths: {
      scratch_root: "/home/me/.local/share/efr/scratch",
      data_dir: "/home/me/.local/share/efr",
    },
    challenge: "ZGVtby1jaGFsbGVuZ2UtMDE",
    prove: {
      peer_node: "nDEMOPHONE1",
      daemon_node: `nDEMO${machine.key.toUpperCase()}`,
      daemon_tailnet: "TDEMO1",
    },
  };
}

/** One socket of the phone to a demo machine. */
export class DemoSession implements Socket {
  readonly machine: DemoMachine;
  private readonly server: DemoServer;
  private readonly listener: SocketListener;
  private mode: MachineMode = "online";
  private state: "opening" | "open" | "closed" = "opening";
  private hellos = 0;
  private outbox: Promise<void> = Promise.resolve();

  constructor(server: DemoServer, machine: DemoMachine, listener: SocketListener) {
    this.server = server;
    this.machine = machine;
    this.listener = listener;
  }

  /** True while the socket is open. */
  get open(): boolean {
    return this.state === "open";
  }

  /** Starts the socket in `mode`. */
  begin(mode: MachineMode): void {
    this.mode = mode;
    this.later(() => {
      if (this.state !== "opening") {
        return;
      }
      if (mode === "offline") {
        this.finish(CLOSE_ABNORMAL);
        return;
      }
      this.state = "open";
      this.listener.opened();
    });
  }

  send(text: string): void {
    if (this.state !== "open") {
      throw new Error("the socket is not open");
    }
    let frame: unknown;
    try {
      frame = JSON.parse(text);
    } catch {
      return;
    }
    if (!isJsonObject(frame)) {
      return;
    }
    this.later(() => this.handle(frame));
  }

  close(): void {
    if (this.state !== "closed") {
      this.state = "closed";
      this.server.closed(this);
    }
  }

  /** The machine drops the socket. */
  drop(): void {
    if (this.state === "closed") {
      return;
    }
    this.later(() => this.finish(CLOSE_ABNORMAL));
  }

  /** Sends one item of the stream `id`. */
  item(id: number, item: WireItem): void {
    this.frame({ id, item });
  }

  private finish(code: number): void {
    if (this.state === "closed") {
      return;
    }
    this.state = "closed";
    this.server.closed(this);
    this.listener.closed(code, "");
  }

  /** Runs `run` after the current call, in the order of the calls. */
  private later(run: () => void): void {
    this.outbox = this.outbox.then(run).catch(() => undefined);
  }

  private frame(frame: Json): void {
    this.later(() => {
      if (this.state === "open") {
        this.listener.message(JSON.stringify(frame));
      }
    });
  }

  private result(id: number, result: unknown): void {
    this.frame({ id, item: result });
    this.frame({ id, end: true });
  }

  private error(id: number, body: ErrorBody): void {
    this.frame({ id, error: body });
  }

  private handle(frame: Json): void {
    if (this.state !== "open" || this.mode === "silent") {
      return;
    }
    const cancel = frame["cancel"];
    if (typeof cancel === "number") {
      for (const conversation of this.server.conversations(this.machine.key)) {
        conversation.unsubscribe(this, cancel);
      }
      return;
    }
    const id = frame["id"];
    const method = frame["method"];
    const params = isJsonObject(frame["params"]) ? frame["params"] : {};
    if (typeof id !== "number" || typeof method !== "string") {
      return;
    }
    if (method === "hello") {
      this.hello(id);
      return;
    }
    if (this.hellos === 0) {
      this.error(id, { code: "unauthorized", message: "send hello first" });
      return;
    }
    this.method(id, method, params);
  }

  private hello(id: number): void {
    this.hellos += 1;
    if (this.hellos > 1) {
      this.error(id, { code: "conflict", message: "hello was already sent" });
      return;
    }
    switch (this.mode) {
      case "starting":
        this.error(id, {
          code: "busy",
          message: "efrd is starting",
          data: { retry_after_ms: STARTING_RETRY_MS },
        });
        return;
      case "old":
        this.result(id, helloOf(this.machine, PROTOCOL_VERSION - 1));
        return;
      default:
        this.result(id, helloOf(this.machine, PROTOCOL_VERSION));
    }
  }

  private method(id: number, method: string, params: Json): void {
    switch (method) {
      case "device.enroll":
        this.enroll(id, params);
        return;
      case "device.prove":
        if (this.mode === "repair") {
          this.error(id, {
            code: "unauthorized",
            message: "this device is not known",
            data: { re_pair: true },
          });
        } else {
          this.result(id, { scopes: SCOPES });
        }
        return;
      case "conversations.recent":
        this.result(id, { conversations: this.recent(numberOf(params["limit"]) ?? 20) });
        return;
      case "conversations.search":
        this.result(id, { rows: this.search(stringOf(params["query"]) ?? "") });
        return;
      case "conversations.list":
        this.result(id, {
          conversations: this.server
            .conversations(this.machine.key)
            .map((conversation) => conversation.log.summary()),
        });
        return;
      case "conversation.history":
        this.result(id, { events: [] });
        return;
      case "conversation.subscribe":
        this.subscribe(id, params);
        return;
      default:
        this.write(id, method, params);
    }
  }

  private enroll(id: number, params: Json): void {
    if (params["token"] !== this.machine.token) {
      this.error(id, { code: "unauthorized", message: "the pairing token is wrong or used" });
      return;
    }
    const deviceId = this.server.newId();
    this.server.clock.setTimer(this.server.confirmMs, () => {
      this.result(id, { device_id: deviceId, scopes: SCOPES });
    });
  }

  private recent(limit: number): RecentConversation[] {
    return this.server
      .conversations(this.machine.key)
      .slice(0, limit)
      .map((conversation) => conversation.recent());
  }

  private search(query: string): Json[] {
    const words = query.toLowerCase().trim();
    return this.server
      .conversations(this.machine.key)
      .filter(
        (conversation) =>
          words === "" || conversation.log.facts.title.toLowerCase().includes(words),
      )
      .map((conversation) => ({
        conversation: conversation.recent(),
        short_id: conversation.id.slice(0, 8),
      }));
  }

  private subscribe(id: number, params: Json): void {
    const conversation = this.server.conversation(stringOf(params["conversation_id"]) ?? "");
    if (conversation === undefined || conversation.machine.key !== this.machine.key) {
      this.error(id, { code: "not_found", message: "no such conversation" });
      return;
    }
    conversation.subscribe(this, id, numberOf(params["after_seq"]));
  }

  /** The writes of the phone: each needs a conversation of this machine. */
  private write(id: number, method: string, params: Json): void {
    const commandId = stringOf(params["command_id"]) ?? "";
    if (method === "prompt.send" && params["new_conversation"] === true) {
      const text = stringOf(params["text"]) ?? "";
      const conversation = this.server.startConversation(this.machine, text.slice(0, 60));
      const sent = conversation.prompt(text, commandId);
      const result = {
        conversation_id: conversation.id,
        queued: false,
        seq: sent.seq,
        turn_id: sent.turnId,
      };
      conversation.remember(commandId, result);
      this.result(id, result);
      return;
    }
    const conversation = this.server.conversation(stringOf(params["conversation_id"]) ?? "");
    if (conversation === undefined || conversation.machine.key !== this.machine.key) {
      this.error(id, { code: "not_found", message: "no such conversation" });
      return;
    }
    const earlier = conversation.earlier(commandId);
    if (earlier !== undefined) {
      this.result(id, earlier);
      return;
    }
    const result = this.change(conversation, method, params);
    if (result === undefined) {
      this.error(id, { code: "conflict", message: `${method} does not apply now` });
      return;
    }
    if ("code" in result) {
      this.error(id, { code: "invalid", message: `the demo machine has no ${method}` });
      return;
    }
    conversation.remember(commandId, result);
    this.result(id, result);
  }

  private change(
    conversation: ServedConversation,
    method: string,
    params: Json,
  ): Json | { readonly code: "invalid" } | undefined {
    switch (method) {
      case "prompt.send": {
        const sent = conversation.prompt(
          stringOf(params["text"]) ?? "",
          stringOf(params["command_id"]) ?? "",
        );
        return {
          conversation_id: conversation.id,
          queued: sent.queued,
          seq: sent.seq,
          turn_id: sent.turnId,
        };
      }
      case "approval.respond": {
        const decision = params["decision"] === "deny" ? "deny" : "allow";
        const seq = conversation.answer(stringOf(params["call_id"]) ?? "", decision);
        return seq === undefined ? undefined : { seq };
      }
      case "turn.interrupt": {
        const stopped = conversation.interrupt();
        return stopped === undefined ? undefined : { seq: stopped.seq, turn_id: stopped.turnId };
      }
      case "turn.steer": {
        const text = stringOf(params["text"]) ?? "";
        const steered = conversation.steer(text);
        if (steered !== undefined) {
          return { seq: steered.seq, turn_id: steered.turnId };
        }
        if (!isJsonObject(params["if_late"])) {
          return undefined;
        }
        const sent = conversation.prompt(text, stringOf(params["command_id"]) ?? "");
        return { queued: true, seq: sent.seq, turn_id: sent.turnId };
      }
      case "prompt.withdraw": {
        const target = params["target"];
        const turn = isJsonObject(target) ? stringOf(target["turn_id"]) : undefined;
        const withdrawn = turn === undefined ? undefined : conversation.withdraw(turn);
        return withdrawn === undefined || turn === undefined
          ? undefined
          : { withdrawn: { seq: withdrawn.seq, text: withdrawn.text, turn_id: turn } };
      }
      default:
        return { code: "invalid" };
    }
  }
}

function stringOf(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function numberOf(value: unknown): number | undefined {
  return typeof value === "number" ? value : undefined;
}
