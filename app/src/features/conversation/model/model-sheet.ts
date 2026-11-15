// The model sheet (S-ModelSheet, MS-Chosen, MS-SignedOut, AC-Out 5, AC-Limits 1-2) and
// the model button of the composer, as pure functions of what they show.
//
// - Sections per account, in the order of `catalogs` in `models.list`, each headed by
//   the account's name (model/accounts.ts). A signed-out account dims its rows and says
//   "signed out on desktop". From 80 % the header has the warning glyph and "5-hour 92%";
//   at 100 % it says "limit reached · resets 14:20 · 12m ago" and its rows dim. A dim
//   row cannot be picked.
// - A row is logo, name and a check. The check is on what the next message uses now:
//   the choice of the sheet, else the model of the conversation, else the machine's
//   default. It moves only on confirm. A model that the machine lists no more is the one
//   row with words: "gone from desktop".
// - ONE effort slider for the selected model: its steps are the model's efforts, a dot
//   marks the default, and selecting a model moves the thumb to its effort here, else
//   its default. A model without efforts has no slider; the quiet line says so.
// - The quiet line is the only place for the context window and the default:
//   "Opus 5.5 · 200k context · desktop default", "chosen here" with "Use default", and
//   "on ChatGPT plan · b" for a model that runs on two accounts.
// - The confirm is the native header button "Use", on only when something changed. Its
//   VoiceOver label says what it uses: "Use Sonnet 5 · medium", or "Use ChatGPT plan · b"
//   for a switch of account alone. While a turn runs, the quiet line says "next
//   message". A switch that compacts, while the phone asks before a costly switch, says
//   "Compacts this conversation and starts a cold cache.", and the button reads "Switch"
//   with the label "Switch and compact". A cold cache alone stays silent.
//
// Round 7 (S-ModelSheet r7, S-ComposerR7):
// - Search: each word of the query must be in the name or the id of the model, in any
//   case. An account with no match goes; no match at all says `no model matches "xyz"`.
// - The filter: one kind of provider (All, Claude, OpenAI) and "Legacy models". A model
//   that efrd marks legacy stands under "Legacy" at the end of its account, and only
//   while "Legacy models" is on (the user, after the device check: hidden by default,
//   shown only by the filter; a search with the filter off does not find them).
// - One favourite for each account: a star at the right end of each row that takes a
//   tap. The favourite goes first in its account, by the favourites of the time when the
//   sheet opened, so the rows never move while it is open. A new thread starts on the
//   last plan of the machine with its favourite (`freshChoice`).
// - The model button has the logo of its account, dim when the account is signed out.
//
// Pure TypeScript.

import type {
  CatalogStatus,
  ModelInfo,
  ModelsListResult,
  SwitchCost,
} from "@/lib/protocol/generated";
import { holdsEveryWord, searchWords } from "@/lib/text/search-words";
import {
  accountName,
  accountShort,
  accountSubject,
  LIMIT_REACHED,
  labelOf,
  limitLevel,
  limitsNow,
  providerLogo,
  type ProviderLogoKind,
  reachedWords,
  runsOnMany,
  signedOutOn,
} from "./accounts";
import { favouriteOf, type ModelChoice } from "./model-choices";
import { modelName, windowLabel } from "./model-names";

/** What the next message uses now, and where it comes from. */
export interface CurrentSettings {
  readonly model: string;
  readonly provider: string | undefined;
  readonly effort: string | undefined;
  /**
   * The choice of the sheet, the model of the conversation, the favourite of the plan of
   * a new thread, or the machine's default.
   */
  readonly source: "choice" | "turn" | "favourite" | "default";
}

/** The settings of the newest turn that names a model. */
export interface TurnSettingsView {
  readonly model: string | undefined;
  readonly provider: string | undefined;
  readonly effort: string | undefined;
}

/** The row of the sheet that `selection` names: a model on an account, and an effort. */
export interface Selection {
  readonly model: string;
  readonly provider: string | undefined;
  readonly effort: string | undefined;
}

/** The models of `list`, or none. */
function modelsOf(list: ModelsListResult | undefined): readonly ModelInfo[] {
  return list?.models ?? [];
}

/** The catalogs of `list`; an old daemon sends only `catalog`. */
function catalogsOf(list: ModelsListResult | undefined): readonly CatalogStatus[] {
  if (list?.catalogs !== undefined) {
    return list.catalogs;
  }
  const one = list?.catalog ?? undefined;
  return one === undefined ? [] : [one];
}

/** The entry of `model` on `provider`; with no provider, the first entry of the model. */
export function findEntry(
  models: readonly ModelInfo[],
  model: string,
  provider: string | undefined,
): ModelInfo | undefined {
  return models.find(
    (entry) =>
      entry.id === model && (provider === undefined || (entry.provider ?? undefined) === provider),
  );
}

/** The model that a turn uses when the prompt names none. */
export function defaultEntry(list: ModelsListResult | undefined): ModelInfo | undefined {
  return modelsOf(list).find((entry) => entry.default === true);
}

/**
 * The effort that the thumb of `entry` shows for `effort`: the effort when the model has
 * it, else the model's default, else the middle step. A model without efforts has none.
 */
export function normalEffort(
  entry: ModelInfo | undefined,
  effort: string | undefined,
): string | undefined {
  const efforts = entry?.efforts ?? [];
  if (efforts.length === 0) {
    return undefined;
  }
  if (effort !== undefined && efforts.includes(effort)) {
    return effort;
  }
  const fallback = entry?.default_effort ?? undefined;
  if (fallback !== undefined && efforts.includes(fallback)) {
    return fallback;
  }
  return efforts[Math.floor((efforts.length - 1) / 2)];
}

/** The favourite of an account, by its provider id. */
export type FavouriteOf = (provider: string | undefined) => string | undefined;

/**
 * The choice that a new thread starts with: the favourite of `plan`, the last plan of the
 * machine, else of the account of the machine's default model, with the model's default
 * effort. None when that account has no favourite in the list: efrd then runs its
 * default.
 */
export function freshChoice(
  list: ModelsListResult | undefined,
  plan: string | undefined,
  favourite: FavouriteOf,
): ModelChoice | undefined {
  const models = modelsOf(list);
  const known = plan !== undefined && models.some((entry) => entry.provider === plan);
  const provider = known ? plan : (defaultEntry(list)?.provider ?? undefined);
  const model = favourite(provider);
  if (model === undefined || findEntry(models, model, provider) === undefined) {
    return undefined;
  }
  return provider === undefined ? { model } : { model, provider };
}

/**
 * What the next message uses: the choice, else the conversation's model, else the
 * favourite of a new thread (`fresh`), else the default.
 */
export function currentSettings(
  list: ModelsListResult | undefined,
  choice: ModelChoice | undefined,
  turn: TurnSettingsView | undefined,
  fresh?: ModelChoice,
): CurrentSettings | undefined {
  const models = modelsOf(list);
  const resolve = (
    model: string,
    provider: string | undefined,
    effort: string | undefined,
    source: CurrentSettings["source"],
  ): CurrentSettings => {
    const entry = findEntry(models, model, provider);
    return {
      model,
      provider: provider ?? entry?.provider ?? undefined,
      effort: entry === undefined ? effort : normalEffort(entry, effort),
      source,
    };
  };
  if (choice !== undefined) {
    return resolve(choice.model, choice.provider, choice.effort, "choice");
  }
  if (turn?.model !== undefined) {
    return resolve(turn.model, turn.provider, turn.effort, "turn");
  }
  if (fresh !== undefined) {
    return resolve(fresh.model, fresh.provider, fresh.effort, "favourite");
  }
  const fallback = defaultEntry(list);
  if (fallback === undefined) {
    return undefined;
  }
  return resolve(
    fallback.id,
    fallback.provider ?? undefined,
    fallback.default_effort ?? undefined,
    "default",
  );
}

/** What picks the model of the next prompt of one composer. */
export interface NextChoiceInput {
  readonly list: ModelsListResult | undefined;
  /** The daemon id of the machine of the composer. */
  readonly machine: string;
  /** True on the blank root: the next prompt starts a new thread. */
  readonly isBlankRoot: boolean;
  /** The choice of the sheet for this composer. */
  readonly choice: ModelChoice | undefined;
  /** The favourites of every machine (`ModelChoicesState.favourites`). */
  readonly favourites: ReadonlyMap<string, string>;
  /** The last plan of the machine. */
  readonly plan: string | undefined;
}

/** The favourite of the last plan, for a new thread only (`freshChoice`). */
function freshOf(input: NextChoiceInput): ModelChoice | undefined {
  if (!input.isBlankRoot) {
    return undefined;
  }
  return freshChoice(input.list, input.plan, (provider) =>
    favouriteOf(input.favourites, input.machine, provider),
  );
}

/**
 * The choice that the next prompt of a composer carries: the choice of the sheet, else
 * on a new thread the favourite of the last plan. A send and a retry both use it, so
 * they pick the same model.
 */
export function promptChoice(input: NextChoiceInput): ModelChoice | undefined {
  return input.choice ?? freshOf(input);
}

/**
 * What the next message of a composer uses, for the model button and the sheet: the
 * rule of `promptChoice`, then the model of the conversation (`turn`), then the default.
 */
export function nextSettings(
  input: NextChoiceInput,
  turn: TurnSettingsView | undefined,
): CurrentSettings | undefined {
  return currentSettings(input.list, input.choice, turn, freshOf(input));
}

/** The selection of the row `model` on `provider`: its effort here, else its default. */
export function pickRow(
  list: ModelsListResult | undefined,
  current: CurrentSettings | undefined,
  model: string,
  provider: string | undefined,
): Selection {
  const entry = findEntry(modelsOf(list), model, provider);
  const here = current?.model === model && current.provider === provider;
  return {
    model,
    provider,
    effort: normalEffort(entry, here ? current.effort : undefined),
  };
}

/** The selection when the sheet opens: the current settings, or the row that a link names. */
export function openingSelection(
  list: ModelsListResult | undefined,
  current: CurrentSettings | undefined,
  preselect: { readonly model: string; readonly provider: string | undefined } | undefined,
): Selection | undefined {
  if (preselect !== undefined) {
    return pickRow(list, current, preselect.model, preselect.provider);
  }
  if (current === undefined) {
    return undefined;
  }
  return { model: current.model, provider: current.provider, effort: current.effort };
}

/** True when `selection` is not what the next message uses now. */
export function selectionChanged(
  current: CurrentSettings | undefined,
  selection: Selection | undefined,
): boolean {
  if (selection === undefined) {
    return false;
  }
  return (
    current === undefined ||
    current.model !== selection.model ||
    current.provider !== selection.provider ||
    current.effort !== selection.effort
  );
}

/** The choice that a confirm of `selection` stores. */
export function choiceOf(selection: Selection): ModelChoice {
  const choice: { model: string; provider?: string; effort?: string } = {
    model: selection.model,
  };
  if (selection.provider !== undefined) {
    choice.provider = selection.provider;
  }
  if (selection.effort !== undefined) {
    choice.effort = selection.effort;
  }
  return choice;
}

/** The words after the name of an account in its header. */
export type SectionNote =
  | { readonly kind: "signed_out"; readonly text: string }
  /** From 80 %: the warning glyph in state_warning, the words in text_subtle. */
  | { readonly kind: "warn"; readonly text: string }
  /** At 100 %: the words in state_error. */
  | { readonly kind: "reached"; readonly text: string };

/** One row of the sheet. */
export interface SheetRow {
  readonly key: string;
  readonly model: string;
  readonly provider: string | undefined;
  readonly name: string;
  readonly logo: ProviderLogoKind;
  /** What the next message uses now. */
  readonly checked: boolean;
  readonly selected: boolean;
  /** Dim and not pickable: its account is signed out or full, or the model is gone. */
  readonly disabled: boolean;
  /** The words of a model that the machine lists no more: "gone from desktop". */
  readonly gone: string | undefined;
  readonly label: string;
  /** The favourite of its account: a filled star. */
  readonly favourite: boolean;
  /** The star takes a tap: the row takes one too. A dim favourite keeps its star. */
  readonly starrable: boolean;
  /** The words of the star action for VoiceOver. */
  readonly starLabel: string;
  /**
   * What VoiceOver says after a tap on the star: "GPT-6 Astra is the favourite of the
   * ChatGPT plan", or "The ChatGPT plan has no favourite".
   */
  readonly starResult: string;
}

/** The rows of one account. */
export interface SheetSection {
  readonly key: string;
  /** The account name; none for the one account of an old daemon. */
  readonly title: string | undefined;
  readonly note: SectionNote | undefined;
  readonly dim: boolean;
  readonly rows: readonly SheetRow[];
  /** The legacy models that show, under "Legacy" at the end of the account. */
  readonly legacy: readonly SheetRow[];
}

/** The provider filter of the sheet: every kind, or the models of one kind. */
export type KindFilter = "all" | "claude" | "openai";

/** The filter button and its menu. */
export interface FilterView {
  /** The machine has accounts of two kinds: the menu offers the provider. */
  readonly kinds: boolean;
  readonly kind: KindFilter;
  readonly legacy: boolean;
  /** A filter is on: the filled glyph in text_bright. */
  readonly on: boolean;
  /** The words of the button for VoiceOver. */
  readonly label: string;
  /** The placeholder of the search field: "Search", or "Search Claude models". */
  readonly placeholder: string;
}

/** The effort slider of the selected model. */
export interface EffortView {
  readonly steps: readonly string[];
  readonly index: number;
  /** The step of the model's default effort, marked with a dot. */
  readonly defaultIndex: number | undefined;
}

/** The quiet line under the slider. */
export interface QuietLine {
  readonly text: string;
  /** True when the line ends with the plain button "Use default". */
  readonly useDefault: boolean;
}

/** The confirm: the native header button "Use" or "Switch". */
export interface ConfirmView {
  /** The VoiceOver label of the button: "Use Sonnet 5 · medium", "Switch and compact". */
  readonly label: string;
  /** A turn runs: the pick applies to the next message. */
  readonly next: boolean;
  /** The switch compacts: the warning line, "Switch", "Don't ask again here". */
  readonly compacts: boolean;
  /** The cost is not known yet: the button is off. */
  readonly pending: boolean;
}

/** The whole sheet. */
export interface SheetView {
  readonly sections: readonly SheetSection[];
  /** The line when a search finds nothing: `no model matches "xyz"`. */
  readonly noMatch: string | undefined;
  readonly filter: FilterView;
  readonly effort: EffortView | undefined;
  readonly quiet: QuietLine | undefined;
  readonly confirm: ConfirmView | undefined;
}

/** What the sheet shows from. */
export interface SheetInput {
  readonly list: ModelsListResult | undefined;
  readonly current: CurrentSettings | undefined;
  readonly selection: Selection | undefined;
  readonly machineName: string;
  readonly now: number;
  /** A turn runs: a confirm applies to the next message. */
  readonly running: boolean;
  /** The phone asks before a switch that compacts here. */
  readonly asks: boolean;
  /** The cost of the switch, from `conversation.switch_preview`, when efrd told it. */
  readonly cost: SwitchCost | undefined;
  /** The cost is asked for now. */
  readonly costPending: boolean;
  /** The words of the search field. */
  readonly query: string;
  readonly kind: KindFilter;
  /** The switch "Legacy models" of the filter menu. */
  readonly showLegacy: boolean;
  /** The favourite of each account now: the stars. */
  readonly favourite: FavouriteOf;
  /** The favourite of each account when the sheet opened: the order of the rows. */
  readonly favouriteAtOpen: FavouriteOf;
}

/** No search, no filter and no favourite, as the sheet opens on a new install. */
export const NO_FILTER: Pick<
  SheetInput,
  "query" | "kind" | "showLegacy" | "favourite" | "favouriteAtOpen"
> = {
  query: "",
  kind: "all",
  showLegacy: false,
  favourite: () => undefined,
  favouriteAtOpen: () => undefined,
};

/** The words of each kind of the filter. */
const KIND_WORDS: Readonly<Record<Exclude<KindFilter, "all">, string>> = {
  claude: "Claude",
  openai: "OpenAI",
};

/** The warning line of a switch that compacts. */
export const COMPACTS_TEXT = "Compacts this conversation and starts a cold cache.";

function rowKey(provider: string | undefined, model: string): string {
  return `${provider ?? ""}/${model}`;
}

function noteOf(
  catalog: CatalogStatus | undefined,
  machineName: string,
  now: number,
): { readonly note: SectionNote | undefined; readonly dim: boolean } {
  if (catalog === undefined) {
    return { note: undefined, dim: false };
  }
  if (catalog.signed_out === true) {
    return { note: { kind: "signed_out", text: signedOutOn(machineName) }, dim: true };
  }
  const level = limitLevel(catalog.limits, now);
  if (level.kind === "warn") {
    return {
      note: { kind: "warn", text: `${level.window} ${String(level.percent)}%` },
      dim: false,
    };
  }
  if (level.kind === "reached") {
    const observedAt = limitsNow(catalog.limits, now)?.observedAt;
    return { note: { kind: "reached", text: reachedWords(level, observedAt, now) }, dim: true };
  }
  return { note: undefined, dim: false };
}

/**
 * What the rows of the sheet show from. It holds no effort, no cost and no time, so the
 * screen keeps the rows while only they change (the React Compiler keeps the result of
 * `sheetRows` while each field keeps its value).
 */
export interface RowsInput {
  readonly list: ModelsListResult | undefined;
  readonly current: CurrentSettings | undefined;
  /** The model of the selection: a value, not the selection, whose effort changes. */
  readonly selectedModel: string | undefined;
  readonly selectedProvider: string | undefined;
  readonly machineName: string;
  readonly query: string;
  readonly kind: KindFilter;
  readonly showLegacy: boolean;
  readonly favourite: FavouriteOf;
  readonly favouriteAtOpen: FavouriteOf;
}

/** The rows of one account, before the note of its account dims them. */
export type RowGroup = Omit<SheetSection, "note" | "dim">;

/** The note of each account at `now`, by section key: it changes with the time. */
export type SectionNotes = ReadonlyMap<
  string,
  { readonly note: SectionNote | undefined; readonly dim: boolean }
>;

/** The rows of `input`, without the note of each account. */
function rowsOf(input: SheetInput): RowsInput {
  return {
    list: input.list,
    current: input.current,
    selectedModel: input.selection?.model,
    selectedProvider: input.selection?.provider,
    machineName: input.machineName,
    query: input.query,
    kind: input.kind,
    showLegacy: input.showLegacy,
    favourite: input.favourite,
    favouriteAtOpen: input.favouriteAtOpen,
  };
}

/** The note of each account of `list` at `now` (signed out, near its limit, full). */
export function sectionNotes(
  list: ModelsListResult | undefined,
  machineName: string,
  now: number,
): SectionNotes {
  const notes = new Map<string, { note: SectionNote | undefined; dim: boolean }>();
  for (const catalog of catalogsOf(list)) {
    const key = catalog.provider ?? "";
    if (!notes.has(key)) {
      notes.set(key, noteOf(catalog, machineName, now));
    }
  }
  return notes;
}

/**
 * The sections of the sheet from its rows and the notes of the accounts. Only the rows
 * of a dim account get new objects, so a new minute keeps every other row.
 */
export function sheetSections(groups: readonly RowGroup[], notes: SectionNotes): SheetSection[] {
  return groups.map((group) => {
    const { note, dim } = notes.get(group.key) ?? { note: undefined, dim: false };
    if (!dim) {
      return { ...group, note, dim };
    }
    const words = note?.kind === "signed_out" ? "signed out" : LIMIT_REACHED;
    const dimmed = (row: SheetRow): SheetRow =>
      row.gone === undefined
        ? { ...row, disabled: true, starrable: false, label: `${row.label}, ${words}` }
        : row;
    return { ...group, note, dim, rows: group.rows.map(dimmed), legacy: group.legacy.map(dimmed) };
  });
}

/** The rows of the sheet by account, in the order of the catalogs. */
export function sheetRows(input: RowsInput): RowGroup[] {
  const models = modelsOf(input.list);
  const catalogs = catalogsOf(input.list);
  const order: (string | undefined)[] = [];
  for (const catalog of catalogs) {
    const provider = catalog.provider ?? undefined;
    if (!order.includes(provider)) {
      order.push(provider);
    }
  }
  for (const entry of models) {
    const provider = entry.provider ?? undefined;
    if (!order.includes(provider)) {
      order.push(provider);
    }
  }
  const { current, selectedModel, selectedProvider } = input;
  const words = searchWords(input.query);
  const filtered = words.length > 0 || input.kind !== "all";
  const out: RowGroup[] = [];
  for (const provider of order) {
    if (input.kind !== "all" && providerLogo(provider) !== input.kind) {
      continue;
    }
    const catalog = catalogs.find((one) => (one.provider ?? undefined) === provider);
    const subject =
      provider === undefined ? "this machine" : accountSubject(provider, catalog?.label);
    const favourite = input.favourite(provider);
    const rows: SheetRow[] = [];
    const legacy: SheetRow[] = [];
    for (const entry of models) {
      if ((entry.provider ?? undefined) !== provider) {
        continue;
      }
      if (!rowShows(entry, words, input.showLegacy)) {
        continue;
      }
      const name = modelName(entry.id);
      const checked = current?.model === entry.id && current.provider === provider;
      const selected = selectedModel === entry.id && selectedProvider === provider;
      const starred = favourite === entry.id;
      const label = [name];
      if (checked) {
        label.push("in use");
      }
      if (starred) {
        label.push(`favourite of ${subject}`);
      }
      if (entry.legacy === true) {
        label.push("legacy");
      }
      const row: SheetRow = {
        key: rowKey(provider, entry.id),
        model: entry.id,
        provider,
        name,
        logo: providerLogo(provider),
        checked,
        selected,
        disabled: false,
        gone: undefined,
        label: label.join(", "),
        favourite: starred,
        starrable: true,
        starLabel: starred
          ? `Remove the favourite of ${subject}`
          : `Make ${name} the favourite of ${subject}`,
        starResult: starred
          ? `${subject.charAt(0).toUpperCase()}${subject.slice(1)} has no favourite`
          : `${name} is the favourite of ${subject}`,
      };
      (entry.legacy === true ? legacy : rows).push(row);
    }
    if (filtered && rows.length === 0 && legacy.length === 0) {
      continue;
    }
    const first = input.favouriteAtOpen(provider);
    const title = provider === undefined ? undefined : accountName(provider, catalog?.label);
    out.push({
      key: provider ?? "",
      title,
      rows: favouriteFirst(rows, first),
      legacy: favouriteFirst(legacy, first),
    });
  }
  return withGone(out, input);
}

/** `rows` with the row of `model` first, so a favourite leads its account. */
function favouriteFirst(rows: SheetRow[], model: string | undefined): SheetRow[] {
  const at = rows.findIndex((row) => row.model === model);
  if (at <= 0) {
    return rows;
  }
  const row = rows[at];
  return row === undefined ? rows : [row, ...rows.slice(0, at), ...rows.slice(at + 1)];
}

/** The line of a search that finds nothing. */
export function noMatchOf(input: RowsInput): string | undefined {
  const query = input.query.trim();
  if (query === "") {
    return undefined;
  }
  // NOTE: it builds no row, so the screen can ask it at each key; a gone row shows for
  // any search (`withGone`).
  const words = searchWords(query);
  const shows = modelsOf(input.list).some(
    (entry) =>
      (input.kind === "all" || providerLogo(entry.provider ?? undefined) === input.kind) &&
      rowShows(entry, words, input.showLegacy),
  );
  if (shows || goneCurrent(input) !== undefined) {
    return undefined;
  }
  const kind = input.kind === "all" ? "" : `${KIND_WORDS[input.kind]} `;
  return `no ${kind}model matches "${query}"`;
}

/** True when the search `words` and the legacy filter show the row of `entry`. */
function rowShows(entry: ModelInfo, words: readonly string[], showLegacy: boolean): boolean {
  return (
    (entry.legacy !== true || showLegacy) && holdsEveryWord(words, [modelName(entry.id), entry.id])
  );
}

/** The filter button of the sheet. */
function filterView(input: SheetInput): FilterView {
  const kinds = new Set<string>();
  for (const catalog of catalogsOf(input.list)) {
    kinds.add(providerLogo(catalog.provider ?? undefined));
  }
  for (const entry of modelsOf(input.list)) {
    kinds.add(providerLogo(entry.provider ?? undefined));
  }
  const both = kinds.has("claude") && kinds.has("openai");
  const kind = both ? input.kind : "all";
  const kindWords = kind === "all" ? "all providers" : KIND_WORDS[kind];
  const legacyWords = input.showLegacy ? "legacy models shown" : "legacy models hidden";
  return {
    kinds: both,
    kind,
    legacy: input.showLegacy,
    on: kind !== "all" || input.showLegacy,
    label: `Filter models, ${kindWords}, ${legacyWords}`,
    placeholder: kind === "all" ? "Search" : `Search ${KIND_WORDS[kind]} models`,
  };
}

/** The current model when the machine lists it no more, else undefined. */
function goneCurrent(input: RowsInput): CurrentSettings | undefined {
  const { current } = input;
  const models = modelsOf(input.list);
  if (current === undefined || models.length === 0) {
    return undefined;
  }
  return findEntry(models, current.model, current.provider) === undefined ? current : undefined;
}

/** The groups with a row for the current model when the machine lists it no more. */
function withGone(out: RowGroup[], input: RowsInput): RowGroup[] {
  const current = goneCurrent(input);
  if (current === undefined) {
    return out;
  }
  const name = modelName(current.model);
  const gone: SheetRow = {
    key: rowKey(current.provider, current.model),
    model: current.model,
    provider: current.provider,
    name,
    logo: providerLogo(current.provider),
    checked: true,
    selected: input.selectedModel === current.model && input.selectedProvider === current.provider,
    disabled: true,
    gone: `gone from ${input.machineName}`,
    label: `${name}, in use, gone from ${input.machineName}`,
    favourite: false,
    starrable: false,
    starLabel: "",
    starResult: "",
  };
  const index = out.findIndex((section) => section.key === (current.provider ?? ""));
  if (index < 0) {
    const group: RowGroup = {
      key: `gone:${current.model}`,
      title: undefined,
      rows: [gone],
      legacy: [],
    };
    return [group, ...out];
  }
  const section = out[index];
  if (section === undefined) {
    return out;
  }
  const next = [...out];
  next[index] = { ...section, rows: [gone, ...section.rows] };
  return next;
}

function effortView(
  entry: ModelInfo | undefined,
  effort: string | undefined,
): EffortView | undefined {
  const steps = entry?.efforts ?? [];
  if (steps.length === 0) {
    return undefined;
  }
  const shown = normalEffort(entry, effort);
  const index = shown === undefined ? 0 : Math.max(0, steps.indexOf(shown));
  const fallback = entry?.default_effort ?? undefined;
  const defaultIndex = fallback === undefined ? -1 : steps.indexOf(fallback);
  return { steps, index, defaultIndex: defaultIndex < 0 ? undefined : defaultIndex };
}

function quietLine(input: SheetInput, entry: ModelInfo | undefined): QuietLine | undefined {
  const { selection, current, machineName } = input;
  if (selection === undefined) {
    return undefined;
  }
  const models = modelsOf(input.list);
  const parts = [modelName(selection.model)];
  const window = entry?.context_window ?? undefined;
  if (window !== undefined) {
    parts.push(`${windowLabel(window)} context`);
  }
  if (selection.provider !== undefined && runsOnMany(models, selection.model)) {
    parts.push(
      `on ${accountName(selection.provider, labelOf(catalogsOf(input.list), selection.provider))}`,
    );
  }
  const fallback = defaultEntry(input.list);
  const isDefault =
    fallback !== undefined &&
    fallback.id === selection.model &&
    (fallback.provider ?? undefined) === selection.provider;
  const isCurrent =
    current !== undefined &&
    current.model === selection.model &&
    current.provider === selection.provider;
  let useDefault = false;
  if (isDefault) {
    parts.push(`${machineName} default`);
  } else if (isCurrent && (current.source === "choice" || current.source === "turn")) {
    parts.push("chosen here");
    useDefault = fallback !== undefined;
  }
  if ((entry?.efforts ?? []).length === 0) {
    parts.push("no effort setting");
  }
  return { text: parts.join(" · "), useDefault };
}

function confirmView(input: SheetInput): ConfirmView | undefined {
  const { selection, current } = input;
  if (selection === undefined || !selectionChanged(current, selection)) {
    return undefined;
  }
  const models = modelsOf(input.list);
  const catalogs = catalogsOf(input.list);
  const compacts = input.asks && input.cost === "compacts";
  let label: string;
  if (compacts) {
    label = "Switch and compact";
  } else if (
    current !== undefined &&
    current.model === selection.model &&
    current.effort === selection.effort &&
    current.provider !== selection.provider &&
    selection.provider !== undefined
  ) {
    label = `Use ${accountName(selection.provider, labelOf(catalogs, selection.provider))}`;
  } else {
    const parts = [modelName(selection.model)];
    if (selection.provider !== undefined && runsOnMany(models, selection.model)) {
      parts.push(accountShort(selection.provider, labelOf(catalogs, selection.provider)));
    }
    if (selection.effort !== undefined) {
      parts.push(selection.effort);
    }
    label = `Use ${parts.join(" · ")}`;
  }
  return {
    label,
    next: input.running,
    compacts,
    pending: input.asks && input.costPending,
  };
}

/**
 * The sheet of `input`. The screen gives `shown` from `sheetRows` and `sectionNotes`
 * apart, so a change of the effort, the cost or the minute builds no row again.
 */
export function sheetView(
  input: SheetInput,
  shown: readonly SheetSection[] = sheetSections(
    sheetRows(rowsOf(input)),
    sectionNotes(input.list, input.machineName, input.now),
  ),
): SheetView {
  const selection = input.selection;
  const entry =
    selection === undefined
      ? undefined
      : findEntry(modelsOf(input.list), selection.model, selection.provider);
  return {
    sections: shown,
    noMatch: noMatchOf(rowsOf(input)),
    filter: filterView(input),
    effort: selection === undefined ? undefined : effortView(entry, selection.effort),
    quiet: quietLine(input, entry),
    confirm: confirmView(input),
  };
}

/** The selection with the machine's default model, for "Use default". */
export function defaultSelection(list: ModelsListResult | undefined): Selection | undefined {
  const fallback = defaultEntry(list);
  if (fallback === undefined) {
    return undefined;
  }
  const provider = fallback.provider ?? undefined;
  return {
    model: fallback.id,
    provider,
    effort: normalEffort(fallback, fallback.default_effort ?? undefined),
  };
}

/** The model button of the composer: "GPT-6.1 Sol · b · medium". */
export interface ModelButtonView {
  readonly text: string;
  readonly label: string;
  /**
   * The user chose the model for this conversation: the button is text_primary and its
   * label says so (MS-Switched, AC-Out 6). Else it is text_muted.
   */
  readonly chosen: boolean;
  /** The mark of the account before the name (S-ComposerR7 1): none for another provider. */
  readonly logo: ProviderLogoKind;
  /** The account is signed out: text_subtle, the mark at 40 %. */
  readonly dim: boolean;
}

/**
 * The button of `current`. It names the account when the model runs on two: the label
 * or the id, or the kind for the default account ("GPT-6.1 Sol · ChatGPT plan · medium").
 * VoiceOver hears the same and at the end "favourite of b" for the favourite of a new
 * thread, or "signed out on desktop" (`machineName`) for a dim button (S-Composer r7).
 */
export function modelButton(
  list: ModelsListResult | undefined,
  current: CurrentSettings | undefined,
  machineName?: string,
): ModelButtonView | undefined {
  if (current === undefined) {
    return undefined;
  }
  const name = modelName(current.model);
  const parts = [name];
  let words = `Model: ${name}`;
  if (current.provider !== undefined && runsOnMany(modelsOf(list), current.model)) {
    const account = accountShort(current.provider, labelOf(catalogsOf(list), current.provider));
    parts.push(account);
    words += ` on ${account}`;
  }
  if (current.effort !== undefined) {
    parts.push(current.effort);
    words += `, ${current.effort} effort`;
  }
  const chosen = current.source === "choice";
  if (chosen) {
    words += ", chosen for this conversation";
  }
  const catalog = catalogsOf(list).find((one) => (one.provider ?? undefined) === current.provider);
  if (current.source === "favourite" && current.provider !== undefined) {
    words += `, favourite of ${accountSubject(current.provider, catalog?.label)}`;
  }
  const dim = catalog?.signed_out === true;
  if (dim) {
    words += `, ${machineName === undefined ? "signed out" : signedOutOn(machineName)}`;
  }
  return {
    text: parts.join(" · "),
    label: words,
    chosen,
    logo: providerLogo(current.provider),
    dim,
  };
}
