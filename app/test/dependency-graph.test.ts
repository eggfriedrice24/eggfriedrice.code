// The import rules of the app (plan sections 2.1 and 3), over every file of src/:
//
// - Alias: an import into another unit (a feature, a folder of lib or components, theme,
//   the routes) starts with "@/"; an import inside a unit is relative. A unit is
//   features/<name>, lib/<name>, components/<name>, theme or app.
// - Layers: lib and theme import nothing from features, components or the routes;
//   components import only lib, theme and components; a feature imports lib, theme,
//   components and only the model/, *-queries.ts and use-*.ts of another feature; a
//   route imports only screens/ and model/ of the features (the root _layout.tsx can
//   import more, for the providers); nothing imports a route.
// - No import cycle, and no barrel (index.ts) outside the routes.
// - zod: only src/lib/protocol/generated-zod.ts and model/*-schemas.ts call `z.`.
//
// The rules are pure functions over a list of files, so the tests below also run them
// on small made-up trees.

import { describe, expect, it } from "vitest";

import { sourceFiles, withoutComments } from "./source-files";

/** A file as the rules see it: its path from app/ and its text without comments. */
interface Source {
  readonly path: string;
  readonly code: string;
}

/** One import: the file, the specifier and the file it names (undefined when none). */
interface Edge {
  readonly from: string;
  readonly specifier: string;
  readonly to: string | undefined;
}

const IMPORT = /^\s*(?:import|export)\b[^;]*?\bfrom\s*"([^"]+)"/gm;
const BARE_IMPORT = /^\s*import\s*"([^"]+)"/gm;
const DYNAMIC_IMPORT = /\bimport\(\s*"([^"]+)"\s*\)/g;

/** The specifiers of every import of `code`. */
function specifiers(code: string): string[] {
  const found: string[] = [];
  for (const pattern of [IMPORT, BARE_IMPORT, DYNAMIC_IMPORT]) {
    for (const match of code.matchAll(pattern)) {
      if (match[1] !== undefined) {
        found.push(match[1]);
      }
    }
  }
  return found;
}

/** `path` with "." and ".." segments resolved; a result outside app/ starts with "../". */
function normalize(path: string): string {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (part === "..") {
      if (parts.length === 0 || parts[parts.length - 1] === "..") {
        parts.push("..");
      } else {
        parts.pop();
      }
    } else if (part !== "." && part !== "") {
      parts.push(part);
    }
  }
  return parts.join("/");
}

function dirname(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash === -1 ? "" : path.slice(0, slash);
}

const EXTENSIONS = ["", ".ts", ".tsx", "/index.ts", "/index.tsx"];

/**
 * The file that `specifier` names from `from`, among `known`. A specifier of a package
 * gives undefined; a local one that names no known file gives its plain path.
 */
function resolve(from: string, specifier: string, known: ReadonlySet<string>): string | undefined {
  let base: string;
  if (specifier.startsWith("@/")) {
    base = `src/${specifier.slice(2)}`;
  } else if (specifier.startsWith("./") || specifier.startsWith("../")) {
    base = normalize(`${dirname(from)}/${specifier}`);
  } else {
    return undefined;
  }
  for (const extension of EXTENSIONS) {
    if (known.has(base + extension)) {
      return base + extension;
    }
  }
  return base;
}

/** Every import of `files`, resolved. */
function edges(files: readonly Source[]): Edge[] {
  const known = new Set(files.map((file) => file.path));
  return files.flatMap((file) =>
    specifiers(file.code).map((specifier) => ({
      from: file.path,
      specifier,
      to: resolve(file.path, specifier, known),
    })),
  );
}

/** The unit of a path under src/, or undefined for a path outside src/. */
function unitOf(path: string): string | undefined {
  const parts = path.split("/");
  if (parts[0] !== "src") {
    return undefined;
  }
  const top = parts[1] ?? "";
  if ((top === "features" || top === "lib" || top === "components") && parts.length > 3) {
    return `${top}/${parts[2]}`;
  }
  return top;
}

/** A local import of a file that does not exist. A CSS file and app/modules are known. */
function unresolved(edge: Edge, known: ReadonlySet<string>): boolean {
  return (
    edge.to !== undefined &&
    !known.has(edge.to) &&
    !edge.to.endsWith(".css") &&
    !edge.to.startsWith("modules/")
  );
}

/** The alias rule. */
function aliasViolations(files: readonly Source[]): string[] {
  const known = new Set(files.map((file) => file.path));
  const found: string[] = [];
  for (const edge of edges(files)) {
    if (edge.to === undefined || edge.to.endsWith(".css")) {
      continue;
    }
    if (unresolved(edge, known)) {
      found.push(`${edge.from}: "${edge.specifier}" names no file`);
      continue;
    }
    const sameUnit = unitOf(edge.from) === unitOf(edge.to);
    if (edge.specifier.startsWith("@/") && sameUnit) {
      found.push(`${edge.from}: "${edge.specifier}" is in the same unit: import it relative`);
    }
    if (!edge.specifier.startsWith("@/") && !sameUnit && !edge.to.startsWith("modules/")) {
      found.push(`${edge.from}: "${edge.specifier}" is in another unit: import it with "@/"`);
    }
  }
  return found;
}

/** Why `from` may not import `to`, or undefined when it may. */
function layerViolation(from: string, to: string): string | undefined {
  const source = from.split("/");
  const target = to.split("/");
  if (target[0] !== "src") {
    return undefined;
  }
  if (target[1] === "app") {
    return "nothing imports a route";
  }
  switch (source[1]) {
    case "lib":
    case "theme":
      return target[1] === "features" || target[1] === "components"
        ? "lib and theme import nothing from features or components"
        : undefined;
    case "components":
      return target[1] === "features" ? "components import nothing from features" : undefined;
    case "features": {
      if (target[1] !== "features" || target[2] === source[2]) {
        return undefined;
      }
      const inner = target.slice(3);
      const name = inner[inner.length - 1] ?? "";
      const open =
        inner[0] === "model" ||
        (inner.length === 1 &&
          (/-queries(?:\.ts)?$/.test(name) || /^use-[^.]*(?:\.ts)?$/.test(name)));
      return open
        ? undefined
        : "a feature imports only the model/, *-queries.ts and use-*.ts of another feature";
    }
    case "app": {
      if (from === "src/app/_layout.tsx") {
        return undefined;
      }
      const open = target[1] === "features" && (target[3] === "screens" || target[3] === "model");
      return open ? undefined : "a route imports only the screens/ and model/ of the features";
    }
    default:
      return undefined;
  }
}

/** The layer rules. */
function layerViolations(files: readonly Source[]): string[] {
  const found: string[] = [];
  for (const edge of edges(files)) {
    if (edge.to === undefined) {
      continue;
    }
    const why = layerViolation(edge.from, edge.to);
    if (why !== undefined) {
      found.push(`${edge.from}: "${edge.specifier}": ${why}`);
    }
  }
  return found;
}

/** The import cycles of `files`, each as its files in order (Tarjan's algorithm). */
function cycles(files: readonly Source[]): string[][] {
  const known = new Set(files.map((file) => file.path));
  const next = new Map<string, string[]>();
  for (const edge of edges(files)) {
    if (edge.to !== undefined && known.has(edge.to)) {
      next.set(edge.from, [...(next.get(edge.from) ?? []), edge.to]);
    }
  }
  const index = new Map<string, number>();
  const low = new Map<string, number>();
  const stack: string[] = [];
  const onStack = new Set<string>();
  const found: string[][] = [];
  const visit = (node: string): void => {
    index.set(node, index.size);
    low.set(node, index.get(node) ?? 0);
    stack.push(node);
    onStack.add(node);
    for (const to of next.get(node) ?? []) {
      if (!index.has(to)) {
        visit(to);
        low.set(node, Math.min(low.get(node) ?? 0, low.get(to) ?? 0));
      } else if (onStack.has(to)) {
        low.set(node, Math.min(low.get(node) ?? 0, index.get(to) ?? 0));
      }
    }
    if (low.get(node) === index.get(node)) {
      const component: string[] = [];
      let member: string | undefined;
      do {
        member = stack.pop();
        if (member !== undefined) {
          onStack.delete(member);
          component.unshift(member);
        }
      } while (member !== undefined && member !== node);
      const selfLoop = (next.get(node) ?? []).includes(node);
      if (component.length > 1 || selfLoop) {
        found.push(component);
      }
    }
  };
  for (const file of files) {
    if (!index.has(file.path)) {
      visit(file.path);
    }
  }
  return found;
}

/** The files that are barrels: an index.ts or index.tsx outside the routes. */
function barrels(files: readonly Source[]): string[] {
  return files
    .map((file) => file.path)
    .filter((path) => /\/index\.tsx?$/.test(path) && !path.startsWith("src/app/"));
}

const Z_CALL = /(?:^|[^\w$.])z\s*\.\s*[A-Za-z_$][\w$]*\s*(?:<[^>()]*>)?\s*\(/;

/** True for a file that may call `z.`: the generated schemas and model/*-schemas.ts. */
function mayCallZod(path: string): boolean {
  return path === "src/lib/protocol/generated-zod.ts" || /\/model\/[^/]+-schemas\.ts$/.test(path);
}

/** The files that call `z.` but may not. */
function zodViolations(files: readonly Source[]): string[] {
  return files
    .filter((file) => !mayCallZod(file.path) && Z_CALL.test(file.code))
    .map((file) => file.path);
}

/** A made-up tree: each entry is a path and its imports, one specifier each. */
function tree(entries: Readonly<Record<string, readonly string[]>>): Source[] {
  return Object.entries(entries).map(([path, imports]) => ({
    path,
    code: imports.map((specifier) => `import { x } from "${specifier}";`).join("\n"),
  }));
}

describe("the dependency graph of the app", () => {
  const files = sourceFiles();

  it("reads the whole tree", () => {
    expect(files.length).toBeGreaterThan(50);
    expect(files.some((file) => file.path === "src/app/_layout.tsx")).toBe(true);
    const local = edges(files).filter((edge) => edge.to?.startsWith("src/") === true);
    expect(local.length).toBeGreaterThan(100);
  });

  it("uses @/ across units and relative paths inside one", () => {
    expect(aliasViolations(files)).toEqual([]);
  });

  it("keeps the layers in their direction", () => {
    expect(layerViolations(files)).toEqual([]);
  });

  it("has no import cycle", () => {
    expect(cycles(files)).toEqual([]);
  });

  it("has no barrel file", () => {
    expect(barrels(files)).toEqual([]);
  });

  it("calls zod only in the schema files", () => {
    expect(zodViolations(files)).toEqual([]);
  });
});

describe("the rules of the dependency graph", () => {
  it("finds an alias across units and a relative path into another unit", () => {
    const found = aliasViolations(
      tree({
        "src/lib/net/a.ts": ["@/lib/net/b", "../protocol/c", "./b"],
        "src/lib/net/b.ts": [],
        "src/lib/protocol/c.ts": ["./missing"],
      }),
    );
    expect(found).toEqual([
      'src/lib/net/a.ts: "@/lib/net/b" is in the same unit: import it relative',
      'src/lib/net/a.ts: "../protocol/c" is in another unit: import it with "@/"',
      'src/lib/protocol/c.ts: "./missing" names no file',
    ]);
  });

  it("allows relative imports of the local Expo modules and of CSS", () => {
    const found = aliasViolations(
      tree({
        "src/features/pairing/native/keys.ts": ["../../../../modules/efr-device-key"],
        "src/app/_layout.tsx": ["../../global.css"],
      }),
    );
    expect(found).toEqual([]);
  });

  it("finds imports against the layers", () => {
    const found = layerViolations(
      tree({
        "src/lib/net/a.ts": ["@/features/machines/model/m"],
        "src/theme/t.ts": ["@/components/ui/c"],
        "src/components/ui/c.tsx": ["@/features/machines/model/m"],
        "src/features/settings/screens/s.tsx": [
          "@/features/machines/model/m",
          "@/features/machines/machine-queries",
          "@/features/machines/use-machines",
          "@/features/machines/components/menu",
          "@/features/machines/screens/x",
          "@/features/machines/native/n",
        ],
        "src/app/settings.tsx": [
          "@/features/settings/screens/s",
          "@/features/settings/model/schemas",
          "@/components/ui/c",
        ],
        "src/app/_layout.tsx": ["@/features/machines/use-machines", "@/app/settings"],
      }),
    );
    expect(found).toEqual([
      'src/lib/net/a.ts: "@/features/machines/model/m": lib and theme import nothing from features or components',
      'src/theme/t.ts: "@/components/ui/c": lib and theme import nothing from features or components',
      'src/components/ui/c.tsx: "@/features/machines/model/m": components import nothing from features',
      'src/features/settings/screens/s.tsx: "@/features/machines/components/menu": a feature imports only the model/, *-queries.ts and use-*.ts of another feature',
      'src/features/settings/screens/s.tsx: "@/features/machines/screens/x": a feature imports only the model/, *-queries.ts and use-*.ts of another feature',
      'src/features/settings/screens/s.tsx: "@/features/machines/native/n": a feature imports only the model/, *-queries.ts and use-*.ts of another feature',
      'src/app/settings.tsx: "@/components/ui/c": a route imports only the screens/ and model/ of the features',
      'src/app/_layout.tsx: "@/app/settings": nothing imports a route',
    ]);
  });

  it("finds a cycle and a self import", () => {
    const found = cycles(
      tree({
        "src/lib/a/a.ts": ["./b"],
        "src/lib/a/b.ts": ["./c"],
        "src/lib/a/c.ts": ["./a", "./d"],
        "src/lib/a/d.ts": [],
        "src/lib/a/e.ts": ["./e"],
      }),
    );
    expect(found).toEqual([
      ["src/lib/a/a.ts", "src/lib/a/b.ts", "src/lib/a/c.ts"],
      ["src/lib/a/e.ts"],
    ]);
  });

  it("finds a barrel outside the routes", () => {
    expect(barrels(tree({ "src/lib/net/index.ts": [], "src/app/index.tsx": [] }))).toEqual([
      "src/lib/net/index.ts",
    ]);
  });

  it("finds a zod call outside the schema files, but not a type or a comment", () => {
    const files: Source[] = [
      { path: "src/lib/protocol/generated-zod.ts", code: "export const a = z.object({});" },
      { path: "src/features/machines/model/machine-schemas.ts", code: "const b = z.guid();" },
      { path: "src/features/machines/model/registry.ts", code: "const c = z.string();" },
      { path: "src/features/machines/model/types.ts", code: "type T = z.infer<typeof a>;" },
      {
        path: "src/features/machines/model/note.ts",
        code: withoutComments("// z.object() is not called here\nconst d = baz.call();", false),
      },
    ];
    expect(zodViolations(files)).toEqual(["src/features/machines/model/registry.ts"]);
  });

  it("strips comments but keeps strings, templates and regular expressions", () => {
    const code = [
      'const url = "ws://host/"; // a comment',
      "const fence = /^`{3,}/; /* a block",
      'comment */ const quote = /"/.test(x);',
      "const t = `a // b`;",
      "return /'/;",
    ].join("\n");
    expect(withoutComments(code, false)).toBe(
      [
        'const url = "ws://host/"; ',
        "const fence = /^`{3,}/; ",
        ' const quote = /"/.test(x);',
        "const t = `a // b`;",
        "return /'/;",
      ].join("\n"),
    );
    const jsx = "<Text>Don't stop</Text>;\nuseEffect(run);";
    expect(withoutComments(jsx, true)).toBe(jsx);
  });

  it("reads imports over several lines, re-exports and dynamic imports", () => {
    const code = [
      "import {",
      "  a,",
      '} from "./a";',
      'export { b } from "./b";',
      'import "./c.css";',
      'type T = typeof import("./d");',
    ].join("\n");
    expect([...specifiers(code)].sort()).toEqual(["./a", "./b", "./c.css", "./d"]);
  });
});
