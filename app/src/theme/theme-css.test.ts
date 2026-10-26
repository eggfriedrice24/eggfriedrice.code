import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { RADII, SPACING } from "./metrics";
import { roles } from "./roles";
import { kebab, themeCss } from "./theme-css";
import { TYPE } from "./type";

const APP = join(import.meta.dirname, "..", "..");
const THEME_CSS = join(import.meta.dirname, "theme.css");
const GLOBAL_CSS = join(APP, "global.css");

describe("theme.css", () => {
  it("is what scripts/gen-theme.mjs writes (run `pnpm gen:theme` after a change)", () => {
    expect(readFileSync(THEME_CSS, "utf8")).toBe(themeCss());
  });

  it("passes the check of scripts/gen-theme.mjs, which runs the theme in Node", () => {
    const run = spawnSync(
      process.execPath,
      ["--disable-warning=MODULE_TYPELESS_PACKAGE_JSON", "scripts/gen-theme.mjs", "--check"],
      { cwd: APP, encoding: "utf8" },
    );
    expect(run.stderr).toBe("");
    expect(run.status).toBe(0);
  });

  it("is imported by global.css after Tailwind and uniwind", () => {
    const imports = [...readFileSync(GLOBAL_CSS, "utf8").matchAll(/@import "([^"]+)";/g)].map(
      (match) => match[1],
    );
    expect(imports).toEqual(["tailwindcss", "uniwind", "./src/theme/theme.css"]);
  });

  it("removes the default tokens of Tailwind before it adds its own", () => {
    const css = themeCss();
    for (const namespace of ["--color-*", "--text-*", "--font-*", "--radius-*"]) {
      expect(css).toContain(`  ${namespace}: initial;`);
    }
    expect(css).toContain("  --spacing: 1px;");
  });

  it("has a token for each role, radius, distance and style", () => {
    const css = themeCss();
    for (const [name, value] of Object.entries(roles)) {
      expect(css).toContain(`  --color-${kebab(name)}: ${value};`);
    }
    for (const [name, value] of Object.entries(RADII)) {
      expect(css).toContain(`  --radius-${kebab(name)}: ${value}px;`);
    }
    for (const [name, value] of Object.entries(SPACING)) {
      expect(css).toContain(`  --spacing-${kebab(name)}: ${value}px;`);
    }
    for (const name of Object.keys(TYPE)) {
      expect(css).toContain(`@utility text-${kebab(name)} {`);
    }
    expect(css).toContain("@utility border-hairline {\n  border-width: 0.5px;\n}");
  });

  it("gives no text style the name of a colour, so each text-* class means one thing", () => {
    const colours = new Set(Object.keys(roles).map(kebab));
    expect(
      Object.keys(TYPE)
        .map(kebab)
        .filter((name) => colours.has(name)),
    ).toEqual([]);
  });

  it("puts no arbitrary value in brackets", () => {
    expect(themeCss()).not.toMatch(/\[/);
  });
});

describe("kebab", () => {
  it("turns role and style names into class names", () => {
    expect(kebab("surface_1")).toBe("surface-1");
    expect(kebab("markdown_link_text")).toBe("markdown-link-text");
    expect(kebab("monoXsTight")).toBe("mono-xs-tight");
    expect(kebab("heading1")).toBe("heading-1");
    expect(kebab("fieldMax")).toBe("field-max");
  });
});
