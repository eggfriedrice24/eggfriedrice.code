// The source files of the app, for the tests that check the whole tree (the dependency
// graph, the effects budget, the React Compiler). It reads the files with node:fs on
// each run, so a new file is always seen.

import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/** The app directory (app/). */
export const APP_DIR = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

/** One source file: its path from app/ with "/" separators, and its text. */
export interface SourceFile {
  readonly path: string;
  readonly text: string;
  /** The text without its comments (`withoutComments`). */
  readonly code: string;
}

/** Every .ts and .tsx file under app/src, sorted by path. */
export function sourceFiles(): SourceFile[] {
  const files: SourceFile[] = [];
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else if (/\.tsx?$/.test(entry.name)) {
        const relative = path.relative(APP_DIR, full).split(path.sep).join("/");
        const text = readFileSync(full, "utf8");
        const code = withoutComments(text, entry.name.endsWith(".tsx"));
        files.push({ path: relative, text, code });
      }
    }
  };
  walk(path.join(APP_DIR, "src"));
  files.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  return files;
}

/** True for a test file (`*.test.ts` or `*.test.tsx`). */
export function isTest(file: string): boolean {
  return /\.test\.tsx?$/.test(file);
}

/** The characters after which a `/` starts a regular expression, not a division. */
const BEFORE_REGEX = new Set(["", "(", ",", "=", ":", "[", "!", "&", "|", "?", "{", "}", ";"]);

/** True when a `/` after `before` starts a regular expression, not a division. */
function startsRegex(before: string): boolean {
  const tail = before.slice(-24).trimEnd();
  return BEFORE_REGEX.has(tail.slice(-1)) || /(?:^|[^\w$])(?:return|typeof|case|throw)$/.test(tail);
}

/** The index after the regular expression literal that starts at `start`. */
function regexEnd(text: string, start: number): number {
  let j = start + 1;
  let inClass = false;
  while (j < text.length && text.charAt(j) !== "\n") {
    const char = text.charAt(j);
    if (char === "\\") {
      j += 2;
      continue;
    }
    if (char === "[") {
      inClass = true;
    } else if (char === "]") {
      inClass = false;
    } else if (char === "/" && !inClass) {
      return j + 1;
    }
    j += 1;
  }
  return j;
}

/**
 * `text` without its comments. String, template and regular expression literals stay,
 * so a `//` in a URL does not start a comment. Each removed comment keeps its line
 * breaks. In JSX (`jsx`), text such as "Don't" has a lone `'`, so a `'` does not start a
 * string there; the formatter writes strings with `"`.
 */
export function withoutComments(text: string, jsx: boolean): string {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const char = text.charAt(i);
    const next = text.charAt(i + 1);
    if (char === "/" && next !== "/" && next !== "*" && startsRegex(out)) {
      const stop = regexEnd(text, i);
      out += text.slice(i, stop);
      i = stop;
    } else if (char === "/" && next === "/") {
      const end = text.indexOf("\n", i);
      i = end === -1 ? text.length : end;
    } else if (char === "/" && next === "*") {
      const end = text.indexOf("*/", i + 2);
      const stop = end === -1 ? text.length : end + 2;
      out += text.slice(i, stop).replace(/[^\n]/g, "");
      i = stop;
    } else if (char === '"' || char === "`" || (char === "'" && !jsx)) {
      let j = i + 1;
      while (j < text.length && text.charAt(j) !== char) {
        j += text.charAt(j) === "\\" ? 2 : 1;
      }
      out += text.slice(i, j + 1);
      i = j + 1;
    } else {
      out += char;
      i += 1;
    }
  }
  return out;
}
