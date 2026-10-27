// The bailout ratchet of the React Compiler (plan section 2.4). The test compiles every
// .tsx file and every use-*.ts hook of src/ with @babel/core and the same
// babel-plugin-react-compiler that Metro uses, and collects the errors that make the
// compiler skip a function. In the app such a bailout is silent (the panic threshold of
// babel-preset-expo is the default), so this test makes it visible.
//
// - A file that is not in KNOWN_BAILOUTS must compile without a bailout.
// - A file in KNOWN_BAILOUTS must still bail out; when it compiles, remove it. The list
//   only gets shorter.

import { createRequire } from "node:module";

import { describe, expect, it } from "vitest";

import { isTest, sourceFiles } from "./source-files";

/** The files that bail out today, from app/. Keep it sorted; it only gets shorter. */
const KNOWN_BAILOUTS: readonly string[] = [];

/** One event of the logger of the compiler. */
interface CompilerEvent {
  readonly kind: string;
  readonly fnName?: string | null;
  readonly detail?: { readonly options?: { readonly reason?: string; readonly category?: string } };
}

/** The part of @babel/core that the test uses. */
interface Babel {
  transformSync(
    code: string,
    options: Readonly<Record<string, unknown>>,
  ): { readonly code?: string | null } | null;
}

/** The kinds of event that mean that the compiler skipped a function. */
const BAILOUT_KINDS = new Set(["CompileError", "PipelineError"]);

const require = createRequire(import.meta.url);

/** Loads @babel/core and the compiler only when a test runs. */
function compiler(): { readonly babel: Babel; readonly plugin: string } {
  const babel: Babel = require("@babel/core");
  return { babel, plugin: require.resolve("babel-plugin-react-compiler") };
}

/** The events of the compiler for `code`, and the compiled code. */
function compile(filename: string, code: string): { code: string; events: CompilerEvent[] } {
  const { babel, plugin } = compiler();
  const events: CompilerEvent[] = [];
  const result = babel.transformSync(code, {
    filename,
    babelrc: false,
    configFile: false,
    parserOpts: { plugins: ["jsx", "typescript"] },
    plugins: [
      [
        plugin,
        {
          target: "19",
          panicThreshold: "none",
          logger: {
            logEvent: (_file: string, event: CompilerEvent) => {
              events.push(event);
            },
          },
        },
      ],
    ],
  });
  return { code: result?.code ?? "", events };
}

/** The reasons of the bailouts in `events`, one line each. */
function bailouts(events: readonly CompilerEvent[]): string[] {
  return events
    .filter((event) => BAILOUT_KINDS.has(event.kind))
    .map((event) => {
      const reason = event.detail?.options?.reason ?? event.detail?.options?.category ?? "";
      return `${event.fnName ?? "(anonymous)"}: ${reason}`;
    });
}

/** True for a file that the compiler must cover: a component file or a hook file. */
function covered(file: string): boolean {
  if (isTest(file)) {
    return false;
  }
  const name = file.slice(file.lastIndexOf("/") + 1);
  return file.endsWith(".tsx") || /^use-.*\.ts$/.test(name);
}

// The check compiles every component and hook of src/ in one test: about 2.5 s on a
// desktop and 6 s on a CI runner, which is over the 5 s default of Vitest.
const WHOLE_TREE_TIMEOUT_MS = 30_000;

describe("the React Compiler", () => {
  it(
    "compiles every component and hook of src/, except the known bailouts",
    () => {
      const failed: string[] = [];
      const fixed: string[] = [];
      const files = sourceFiles().filter((source) => covered(source.path));
      expect(files.length).toBeGreaterThan(5);
      for (const file of files) {
        const reasons = bailouts(compile(file.path, file.text).events);
        const known = KNOWN_BAILOUTS.includes(file.path);
        if (reasons.length > 0 && !known) {
          failed.push(`${file.path}: ${reasons.join("; ")}`);
        }
        if (reasons.length === 0 && known) {
          fixed.push(`${file.path}: compiles now; remove it from KNOWN_BAILOUTS`);
        }
      }
      expect([...failed, ...fixed]).toEqual([]);
    },
    WHOLE_TREE_TIMEOUT_MS,
  );

  it("sees a bailout, so the check above can fail", () => {
    const { code, events } = compile(
      "bailout.tsx",
      [
        'import { useRef } from "react";',
        "export function Bad() {",
        "  const ref = useRef(0);",
        "  ref.current += 1;",
        "  return <Text>{ref.current}</Text>;",
        "}",
      ].join("\n"),
    );
    expect(bailouts(events)).not.toEqual([]);
    expect(code).not.toContain("react/compiler-runtime");
  });

  it("covers the components and the hooks, not the tests or the pure files", () => {
    expect(covered("src/features/settings/screens/settings-screen.tsx")).toBe(true);
    expect(covered("src/features/machines/use-machines.ts")).toBe(true);
    expect(covered("src/features/machines/model/registry.ts")).toBe(false);
    expect(covered("src/components/ui/braille-cell.test.tsx")).toBe(false);
    expect([...KNOWN_BAILOUTS].sort()).toEqual(KNOWN_BAILOUTS);
  });
});
