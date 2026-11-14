// The names of the models as the phone shows them (S-ModelSheet): "Opus 5.5" for
// `claude-opus-5-5`, "GPT-6.1 Sol" for `gpt-6.1-sol`, and a context window in short
// words. An id of another form shows as it is.
//
// Pure TypeScript.

const CLAUDE = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$/u;
const GPT = /^gpt-(\d+(?:\.\d+)?)(?:-(.+))?$/u;

function capital(word: string): string {
  return word.length === 0 ? word : `${word[0]?.toUpperCase() ?? ""}${word.slice(1)}`;
}

/** The display name of model `id`, such as "Opus 5.5" or "GPT-5.5 Codex"; else the id. */
export function modelName(id: string): string {
  const claude = CLAUDE.exec(id);
  if (claude !== null) {
    const [, family = "", major = "", minor] = claude;
    return `${capital(family)} ${major}${minor === undefined ? "" : `.${minor}`}`;
  }
  const gpt = GPT.exec(id);
  if (gpt !== null) {
    const [, version = "", rest] = gpt;
    const words = rest === undefined ? [] : rest.split("-").map(capital);
    return [`GPT-${version}`, ...words].join(" ");
  }
  return id;
}

/** A context window in short words: "200k", "1M", "1.5M". */
export function windowLabel(tokens: number): string {
  if (tokens >= 1_000_000) {
    const millions = tokens / 1_000_000;
    return `${Number.isInteger(millions) ? String(millions) : millions.toFixed(1)}M`;
  }
  return `${String(Math.round(tokens / 1000))}k`;
}
