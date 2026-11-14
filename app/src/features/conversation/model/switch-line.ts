// The ⇄ line of a switch of model or account (MS-Switched, S-ModelSheet "Transcript"):
// one quiet mono line above the prompt of the turn that runs on the new model, not at
// the moment of the switch. Nothing to expand, not tappable.
//
// - `⇄ GPT-5.5 · medium · compacted`: the name, the effort, and "compacted" when the
//   turn compacted the conversation first.
// - `⇄ GPT-6.1 Sol on b · medium`: "on" and the account when the model runs on two
//   accounts, or when only the account changed (a switch of account alone always names
//   it). The account is its label or id, or the kind for the default account
//   ("on ChatGPT plan").
//
// Pure TypeScript.

import type { CatalogStatus, ModelInfo } from "@/lib/protocol/generated";
import type { SwitchView } from "@/lib/stream/reducer";
import { accountShort, labelOf, runsOnMany } from "./accounts";
import { modelName } from "./model-names";

/** The line and what VoiceOver reads. */
export interface SwitchLine {
  readonly text: string;
  readonly label: string;
}

/** What the line knows of the machine: its models and accounts, when the app has them. */
export interface SwitchCatalog {
  readonly models: readonly ModelInfo[];
  readonly catalogs: readonly CatalogStatus[];
}

/**
 * The line of `switched`. `turnEffort` is the effort of the turn, for a daemon that did
 * not send the effort of the switch.
 */
export function switchLine(
  switched: SwitchView,
  turnEffort: string | undefined,
  catalog: SwitchCatalog | undefined,
): SwitchLine {
  const { from, to } = switched;
  const name = modelName(to.model);
  const provider = to.provider ?? undefined;
  const accountOnly =
    from.model === to.model && provider !== undefined && (from.provider ?? undefined) !== provider;
  const many = catalog !== undefined && runsOnMany(catalog.models, to.model);
  const account =
    provider !== undefined && (accountOnly || many)
      ? accountShort(provider, labelOf(catalog?.catalogs ?? [], provider))
      : undefined;
  const effort = to.effort ?? to.default_effort ?? turnEffort;
  let head = `⇄ ${name}`;
  let label = `Switched to ${name}`;
  if (account !== undefined) {
    head += ` on ${account}`;
    label += ` on ${account}`;
  }
  const parts = [head];
  if (effort !== undefined) {
    parts.push(effort);
    label += `, ${effort} effort`;
  }
  if (switched.compacted) {
    parts.push("compacted");
    label += ", the conversation was compacted";
  }
  return { text: parts.join(" · "), label };
}
