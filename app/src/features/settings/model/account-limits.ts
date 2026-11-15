// Settings › Accounts on <machine> (AC-Limits 4 to 6): one row per account of the
// machine, named as its header in the model sheet, in the order of `models.list`, and
// one screen per account with a row per limit. No pooled view.
//
// - The value of a row is the fullest limit of the account: nothing below 80 %, the
//   warning glyph and the percent from 80 %, "limit reached" at 100 %. A signed-out
//   account says so under its name and shows no limit.
// - The screen of an account: one row per window ("5-hour", "weekly") or per limit that
//   stops an API key ("requests"), with "limit reached" or the percent, and the reset.
//   The footer gives the age of the values. An API key shows a limit only when it was
//   hit; until then the screen has no row.
//
// Pure TypeScript.

import type { CatalogStatus } from "@/lib/protocol/generated";
import {
  accountName,
  accountKind,
  ageText,
  isoMs,
  limitLevel,
  limitsNow,
  limitWords,
  resetText,
  windowName,
} from "@/features/conversation/model/accounts";

/** The value of an account row. */
export type AccountValue =
  | { readonly kind: "none" }
  /** From 80 %: the warning glyph in state_warning and the percent in text_subtle. */
  | { readonly kind: "warn"; readonly text: string }
  /** At 100 %: "limit reached" in state_error. */
  | { readonly kind: "reached"; readonly text: string };

/** One row of "Accounts on desktop". */
export interface AccountRow {
  readonly provider: string;
  readonly name: string;
  /** "signed out on desktop" under the name, for an account without a login. */
  readonly note: string | undefined;
  readonly value: AccountValue;
  readonly label: string;
}

/** The rows of the accounts of `catalogs`. */
export function accountRows(
  catalogs: readonly CatalogStatus[],
  machineName: string,
  now: number,
): AccountRow[] {
  const rows: AccountRow[] = [];
  for (const catalog of catalogs) {
    const provider = catalog.provider ?? undefined;
    if (provider === undefined) {
      continue;
    }
    const name = accountName(provider, catalog.label);
    if (catalog.signed_out === true) {
      const note = `signed out on ${machineName}`;
      rows.push({ provider, name, note, value: { kind: "none" }, label: `${name}, ${note}` });
      continue;
    }
    const level = limitLevel(catalog.limits, now);
    let value: AccountValue = { kind: "none" };
    if (level.kind === "warn") {
      value = { kind: "warn", text: `${String(level.percent)}%` };
    } else if (level.kind === "reached") {
      value = { kind: "reached", text: "limit reached" };
    }
    const label =
      value.kind === "none"
        ? name
        : `${name}, ${value.kind === "warn" ? `${value.text} used` : value.text}`;
    rows.push({ provider, name, note: undefined, value, label });
  }
  return rows;
}

/** One row of the screen of an account. */
export interface LimitRow {
  readonly key: string;
  /** "5-hour", "weekly", "requests". */
  readonly name: string;
  /**
   * "limit reached" in state_error, or "63%" in text_muted: mono 13, as AC-Limits 5 and
   * 6 draw it.
   */
  readonly value: string;
  /** " · resets 14:20" after the value, in text_subtle; none when the reset is not known. */
  readonly reset: string | undefined;
  readonly reached: boolean;
  /** The VoiceOver label of the row: "5-hour, limit reached, resets 14:20". */
  readonly label: string;
}

/** The screen of one account. */
export interface AccountScreenView {
  readonly title: string;
  readonly header: string;
  readonly rows: readonly LimitRow[];
  readonly footer: string;
}

function limitRow(
  key: string,
  name: string,
  value: string,
  resetsAt: string | null | undefined,
  now: number,
): LimitRow {
  const at = isoMs(resetsAt);
  const words = at === undefined ? undefined : resetText(at, now);
  return {
    key,
    name,
    value,
    reset: words === undefined ? undefined : ` · ${words}`,
    reached: value === LIMIT_REACHED,
    label: [name, value, words].filter((part) => part !== undefined).join(", "),
  };
}

const LIMIT_REACHED = "limit reached";

/** The screen of account `provider`; undefined when the list does not name it. */
export function accountScreen(
  catalogs: readonly CatalogStatus[],
  provider: string,
  machineName: string,
  now: number,
): AccountScreenView | undefined {
  const catalog = catalogs.find((one) => one.provider === provider);
  if (catalog === undefined) {
    return undefined;
  }
  const title = accountName(provider, catalog.label);
  const header = `On ${machineName}`;
  const apiKey = accountKind(provider).endsWith("-api");
  if (catalog.signed_out === true) {
    return {
      title,
      header,
      rows: [],
      footer: `Signed out on ${machineName}. Log in on the machine to use it again.`,
    };
  }
  const held = limitsNow(catalog.limits, now);
  const rows: LimitRow[] = [];
  for (const [index, window] of (held?.windows ?? []).entries()) {
    const reached = window.used_percent >= 100;
    rows.push(
      limitRow(
        `w${String(index)}`,
        windowName(window.minutes),
        reached ? LIMIT_REACHED : `${String(window.used_percent)}%`,
        window.resets_at,
        now,
      ),
    );
  }
  const stop = held?.reached;
  if (stop !== undefined && !(stop.kind === "usage" && rows.some((row) => row.reached))) {
    rows.push(
      limitRow(`r:${stop.kind}`, limitWords(stop.kind), LIMIT_REACHED, stop.resets_at, now),
    );
  }
  const parts: string[] = [];
  const observedAt = held?.observedAt;
  if (observedAt !== undefined) {
    parts.push(`From the last call on this account, ${ageText(observedAt, now)}.`);
  }
  if (rows.some((row) => row.reached)) {
    parts.push('"limit reached" clears after its reset time.');
  }
  if (apiKey) {
    parts.push("An API key shows a rate limit only when it was hit.");
  } else if (rows.length === 0) {
    parts.push("efr learns the limits of a plan from its next call.");
  }
  return { title, header, rows, footer: parts.join(" ") };
}
