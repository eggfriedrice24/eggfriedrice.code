// The words of an exit of the sandbox (auto mode, board AU-Auto and errata 1, 6, 7 and 9
// of round 6), the same words as efr's terminal (crates/efr-cli/src/format/sandbox.rs and
// always.rs). Pure, so each case has a test.
//
// - The line under a tool card is mono 11 in efr's words, wrapped, never cut: the
//   verdict, then after a colon what the call got, on the same line, with `, ` between
//   the grants. `reviewer allowed: git push for this call (low risk; you asked to push
//   the fix): github.com:443 reachable, the variable GH_TOKEN`.
// - While the reviewer judges, the line holds what the call asks for. A call that got
//   nothing beyond the sandbox has no line.
// - A refusal: `reviewer refused: <line>: <reason>`, or `efr: <line>` for efr's own
//   floor, with the floor's text only when efrd sends one (errata 9).
// - No reviewer model answered: `reviewer: no answer (skipped <model>: <why>); nothing ran`.
// - The pause of the reviewer is one quiet line of its turn. It asks nothing; the next
//   action that needs the reviewer stops the turn (errata 6).
// - A change to a git setting or another file that runs code is one quiet line of the
//   card of its call: moved to quarantine, or left in place (Q-Quarantine 7).

import type { Grant, PauseReason, Risk, SurfaceChange } from "@/lib/protocol/generated";
import type {
  ExitCallView,
  ExitVerdictView,
  ReviewerPause,
  ToolCallView,
} from "@/lib/stream/reducer";
import { shortPath } from "@/features/conversations/model/list-format";

/** The words of one grant, such as `~/notes writable` or `github.com:443 reachable`. */
export function grantWords(grant: Grant): string {
  switch (grant.kind) {
    case "write":
      return `${shortPath(grant.path)} writable`;
    case "host":
      return `${oneLine(grant.host)}:${String(grant.port)} reachable`;
    case "open_network":
      return "host network";
    case "socket":
      return `the socket ${shortPath(grant.path)}`;
    case "bus":
      return busWords(grant.bus);
    case "desktop":
      return `the ${DESKTOP_NAMES[grant.desktop] ?? "desktop"} socket`;
    case "device":
      return `the device ${shortPath(grant.path)}`;
    case "unmask":
      return `${shortPath(grant.path)} readable`;
    case "env":
      return `the variable ${oneLine(grant.name)}`;
    case "keyring":
      return "the keyring (every unlocked entry)";
    case "net_view":
      return "host network view (no sending)";
    case "bus_filter":
      return `filtered bus: ${busWords(grant.bus)}`;
    case "unknown":
      return "unknown grant";
    default:
      return "more access";
  }
}

/** The grant that the card marks with the warning glyph: the whole network. */
export function isLoudGrant(grant: Grant): boolean {
  return grant.kind === "open_network";
}

const DESKTOP_NAMES: Readonly<Record<string, string>> = {
  wayland: "Wayland",
  x11: "X11",
  hyprland: "Hyprland",
  sway: "sway",
  i3: "i3",
  niri: "niri",
};

function busWords(bus: string): string {
  switch (bus) {
    case "system":
      return "the system bus";
    case "session":
      return "the session bus";
    default:
      return "a message bus";
  }
}

/** The grants of a call in words, with `, ` between them. */
export function grantList(grants: readonly Grant[]): string {
  return grants.map(grantWords).join(", ");
}

/** `verb` and the paths, or `fallback` when there are none. */
function withPaths(verb: string, paths: readonly string[], fallback: string): string {
  return paths.length === 0 ? fallback : `${verb} ${paths.join(", ")}`;
}

/** One exit kind in words, such as `write ~/.zshrc` or `network (github.com)`. */
function kindPart(kind: string, exit: ExitCallView): string {
  const pathsOf = (pick: Grant["kind"]): string[] => {
    const granted = exit.grants.flatMap((grant) =>
      grant.kind === pick && "path" in grant ? [shortPath(grant.path)] : [],
    );
    return granted.length > 0 ? granted : exit.targets.map(shortPath);
  };
  switch (kind) {
    case "write":
    case "persistence":
      return withPaths("write", pathsOf("write"), kindWords(kind));
    case "synced_write":
      return `${withPaths("write", pathsOf("write"), "write")} (a synced folder)`;
    case "above_root":
      return `${withPaths("write", pathsOf("write"), "write")} (at or above a project root)`;
    case "masked_read":
      return withPaths("read", pathsOf("unmask"), "a masked read");
    case "destructive":
      return withPaths("destroy or overwrite", exit.targets.map(shortPath), "a destructive change");
    case "host":
      return exit.hosts.length === 0
        ? "network"
        : `network (${exit.hosts.map(oneLine).join(", ")})`;
    case "host_view":
      return "a view of the host's network";
    case "socket":
      return withPaths("the socket", pathsOf("socket"), "a socket");
    case "bus": {
      const bus = exit.grants.find((grant) => grant.kind === "bus");
      return bus === undefined ? "a message bus" : grantWords(bus);
    }
    case "device":
      return withPaths("the device", pathsOf("device"), "a device");
    case "outside":
      return outsideProgram(exit) ?? "the whole line";
    case "privilege":
      return `${exit.programs[0] ?? "more rights"} (you may need to type your password)`;
    case "upload":
      return exit.uploads.length === 0 ? "upload" : `upload (${exit.uploads.join(", ")})`;
    case "secret":
      return "a secret (denied)";
    case "config":
      return "efr's config (denied)";
    case "desktop_ipc": {
      const desktop = exit.grants.find((grant) => grant.kind === "desktop");
      return desktop === undefined ? "the desktop" : grantWords(desktop);
    }
    default:
      return kindWords(kind);
  }
}

function kindWords(kind: string): string {
  return kind.replace(/_/g, " ");
}

/** The program of the first command that runs outside the sandbox, such as `df`. */
function outsideProgram(exit: ExitCallView): string | undefined {
  const command = exit.outside[0];
  if (command === undefined) {
    return exit.programs[0];
  }
  return command.split(/\s+/).find((word) => word !== "" && !/^[A-Za-z_]\w*=/.test(word));
}

/** What the exit opens, each kind once: `write ~/notes; network`, or `this call`. */
export function exitWhat(exit: ExitCallView): string {
  const parts: string[] = [];
  for (const kind of exit.kinds) {
    const part = kindPart(kind, exit);
    if (!parts.includes(part)) {
      parts.push(part);
    }
  }
  return parts.length === 0 ? "this call" : parts.join("; ");
}

/** The words of a risk, such as `low risk`. */
function riskWords(risk: Risk): string {
  return `${risk} risk`;
}

/** True for a verdict that no reviewer model gave: every step of the chain failed. */
export function isNoAnswer(judged: ExitVerdictView): boolean {
  return (
    judged.judge === "classifier" &&
    judged.verdict !== "allow" &&
    judged.risk === undefined &&
    (judged.rationale ?? "").trim() === ""
  );
}

/** True for a verdict that kept the call from running, without a question. */
export function isRefused(judged: ExitVerdictView | undefined): boolean {
  return (
    judged !== undefined &&
    judged.verdict === "deny" &&
    (judged.judge === "classifier" || judged.judge === "floor")
  );
}

/** The line under a tool card about its exit. */
export interface ExitNote {
  readonly text: string;
  /** A refusal is text_muted; the rest is text_subtle. */
  readonly tone: "text_subtle" | "text_muted";
  /**
   * The loudest grant in words (`host network`), or `undefined`. The card puts it on a
   * line of its own after `text`, with the warning glyph before it (AU-Auto).
   */
  readonly loud: string | undefined;
}

/**
 * The line under the card of a call with `exit`, or `undefined` when it has none.
 * `stopped` is the pause of the turn when the turn ended while the call waited, so the
 * next action stopped the turn. `text` is empty when the loud grant is all there is.
 */
export function exitNote(
  exit: ExitCallView,
  summary: string,
  stopped: ReviewerPause | undefined,
): ExitNote | undefined {
  const grants = grantList(exit.grants.filter((grant) => !isLoudGrant(grant)));
  const loudGrants = exit.grants.filter(isLoudGrant);
  const loud = loudGrants.length === 0 ? undefined : grantList(loudGrants);
  const judged = exit.judged;
  const subject = oneLine(exit.line.trim() === "" ? summary : exit.line.trim());
  if (stopped !== undefined && judged === undefined) {
    return {
      text: `auto stopped this turn: ${refusedWords(stopped)}`,
      tone: "text_muted",
      loud: undefined,
    };
  }
  const grantsOnly = (): ExitNote | undefined =>
    grants === "" && loud === undefined
      ? undefined
      : {
          text: loud === undefined || grants === "" ? grants : `${grants},`,
          tone: "text_subtle",
          loud,
        };
  if (judged === undefined) {
    return grantsOnly();
  }
  const rationale = oneLine(judged.rationale ?? "").trim();
  // The loud grant goes on the next line, so the verdict line ends with a colon, or with
  // a comma after the other grants.
  const withGrants = (line: string): ExitNote => {
    let text = line;
    if (grants !== "") {
      text = loud === undefined ? `${line}: ${grants}` : `${line}: ${grants},`;
    } else if (loud !== undefined) {
      text = `${line}:`;
    }
    return { text, tone: "text_subtle", loud };
  };
  switch (judged.judge) {
    case "classifier": {
      if (isNoAnswer(judged)) {
        const skipped = judged.skipped.map(oneLine);
        const why = skipped.length === 0 ? "" : ` (skipped ${skipped.join(", ")})`;
        return {
          text: `reviewer: no answer${why}; nothing ran`,
          tone: "text_muted",
          loud: undefined,
        };
      }
      if (judged.verdict === "allow") {
        const what = exitWhat(exit);
        const span = exit.background ? "for the whole job" : "for this call";
        const why: string[] = [];
        if (judged.risk !== undefined) {
          why.push(riskWords(judged.risk));
        }
        if (rationale !== "") {
          why.push(rationale);
        }
        const reasons = why.length === 0 ? "" : ` (${why.join("; ")})`;
        return withGrants(`reviewer allowed: ${what} ${span}${reasons}`);
      }
      if (judged.verdict === "deny") {
        const text =
          rationale === ""
            ? `reviewer refused: ${subject}`
            : `reviewer refused: ${subject}: ${rationale}`;
        return { text, tone: "text_muted", loud: undefined };
      }
      // An unsure verdict hands the call to a question, which shows it.
      return undefined;
    }
    case "always": {
      if (judged.verdict !== "allow") {
        return undefined;
      }
      const place = oneLine(judged.category ?? "").trim();
      const rule = rationale;
      let line = "allowed by your rule";
      if (place !== "" && rule !== "") {
        line = `allowed by your rule ${place}: ${rule}`;
      } else if (place !== "") {
        line = `allowed by your rule ${place}`;
      } else if (rule !== "") {
        line = `allowed by your rule: ${rule}`;
      }
      return withGrants(line);
    }
    case "floor": {
      const text = rationale === "" ? `efr: ${subject}` : `efr: ${subject}: ${rationale}`;
      return { text, tone: "text_muted", loud: undefined };
    }
    default:
      // A person answered (the approval shows it), or full mode let it through.
      return grantsOnly();
  }
}

/**
 * True when the pause of the reviewer stopped the turn at `call`: efrd records the exit
 * of that action with no verdict, and the action does not run. `asked` is true when the
 * call has an approval question, which a person answers.
 */
export function stoppedByPause(
  call: ToolCallView,
  pause: ReviewerPause | undefined,
  asked: boolean,
  turnEnded: boolean,
): boolean {
  return (
    pause !== undefined &&
    !asked &&
    call.exit !== undefined &&
    call.exit.judged === undefined &&
    (call.done || turnEnded)
  );
}

/** `3 actions in a row were refused`, `10 actions of this turn were refused`. */
function refusedWords(pause: ReviewerPause): string {
  return pauseCount(pause.reason, pause.denials);
}

function pauseCount(reason: PauseReason, denials: number): string {
  return reason === "per_turn"
    ? `${String(denials)} actions of this turn were refused`
    : `${String(denials)} actions in a row were refused`;
}

/**
 * The quiet line of the pause of the reviewer: `reviewer paused for this turn: 3 actions
 * in a row were refused; the next action that needs it stops the turn`. The count holds
 * the refusals of the reviewer and of efr's own rules, so it names no one.
 */
export function pausedLine(pause: ReviewerPause): string {
  return `reviewer paused for this turn: ${pauseCount(pause.reason, pause.denials)}; the next action that needs it stops the turn`;
}

/**
 * The row of a question that says what the reviewer thought before it handed the question
 * over, such as `reviewer: unsure; no message of yours names this host`.
 */
export function reviewerRow(verdict: string, rationale: string): string {
  const opinion =
    verdict === "allow"
      ? "it would allow this, but efr's rules ask you"
      : verdict === "deny"
        ? "it would refuse this"
        : "unsure";
  const why = oneLine(rationale).trim();
  return why === "" ? `reviewer: ${opinion}` : `reviewer: ${opinion}; ${why}`;
}

/**
 * What a change to a git setting or another file that runs code does, as efr names it
 * (`SurfaceChange::what` of efr-protocol): the code key it sets, else its rule in words.
 */
export function surfaceWhat(change: SurfaceChange): string {
  if (change.key !== undefined && change.key !== null) {
    return change.key;
  }
  return SURFACE_RULES[change.rule] ?? change.rule.replace(/_/g, " ");
}

const SURFACE_RULES: Readonly<Record<string, string>> = {
  commondir_in_main_git_dir: "a commondir that points git at another config",
  config_worktree_appeared: "a new config.worktree",
  worktree_config_worktree_appeared: "a new config.worktree",
  worktree_commondir_elsewhere: "a commondir that points away from its main git dir",
  config_code_key: "a key that runs a program",
  config_unreadable: "a config that git cannot read",
  commondir_outside_repo: "a commondir that points outside the repository",
  hook_planted: "a new hook",
  module_code_key: "a submodule key that runs a program",
  module_hook: "a new submodule hook",
  module_link: "a link to a submodule git dir that efr cannot pin",
  modules_too_deep: "submodule git dirs deeper than efr checks",
  alternates_changed: "git objects from another place",
  protected_name_created: "a new agent or editor config",
};

/**
 * The quiet lines of the changes that a call made to files that run code (Q-Quarantine
 * 7, the reviewer on): one per change, `.git/config (core.fsmonitor); moved to
 * quarantine` or `.envrc (a new agent or editor config); left in place`. A path in the
 * working directory `cwd` of the turn is relative to it.
 */
export function surfaceLines(changes: readonly SurfaceChange[], cwd: string | undefined): string[] {
  return changes.map((change) => {
    const where = change.quarantined ? "moved to quarantine" : "left in place";
    return `${placeOf(change.path, cwd)} (${oneLine(surfaceWhat(change))}); ${where}`;
  });
}

/** `path` relative to `cwd` when it lies in it, else with `~` for the home directory. */
function placeOf(path: string, cwd: string | undefined): string {
  if (cwd !== undefined && cwd !== "") {
    const root = cwd.endsWith("/") ? cwd : `${cwd}/`;
    if (path.startsWith(root) && path.length > root.length) {
      return path.slice(root.length);
    }
  }
  return shortPath(path);
}

/** `text` on one line: each run of white space, a line break too, is one space. */
function oneLine(text: string): string {
  return text.replace(/\s+/g, " ");
}
