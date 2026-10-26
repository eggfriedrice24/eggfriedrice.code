import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { roles } from "./roles";

const ICON_DIR = join(import.meta.dirname, "..", "..", "assets", "icon");
const ROLE_COLOURS = new Set<string>(Object.values(roles));

const svgs = readdirSync(ICON_DIR)
  .filter((name) => name.endsWith(".svg"))
  .map((name) => [name, readFileSync(join(ICON_DIR, name), "utf8")] as const);

describe("the app icon", () => {
  it("has the base, the three layers and the two appearances", () => {
    expect(svgs.map(([name]) => name).sort()).toEqual([
      "appearance-clear.svg",
      "appearance-tinted.svg",
      "icon.svg",
      "layer-background.svg",
      "layer-egg-white.svg",
      "layer-yolk.svg",
    ]);
  });

  it("uses only colours of the design system, on a 1024 x 1024 canvas", () => {
    const problems = svgs.flatMap(([name, svg]) => {
      const colours = [...svg.matchAll(/#[0-9a-f]{3,8}\b/gi)].map((match) => match[0]);
      const found = colours
        .filter((colour) => !ROLE_COLOURS.has(colour))
        .map((colour) => `${name}: ${colour} is not a role`);
      if (colours.length === 0) {
        found.push(`${name}: no colour`);
      }
      if (!svg.includes('viewBox="0 0 1024 1024"')) {
        found.push(`${name}: not 1024 x 1024`);
      }
      if (/Gradient|filter|<text/.test(svg)) {
        found.push(`${name}: a gradient, a filter or text`);
      }
      return found;
    });
    expect(problems).toEqual([]);
  });

  it("is a 1024 x 1024 PNG with no alpha channel", () => {
    const png = readFileSync(join(ICON_DIR, "icon.png"));
    expect(png.subarray(0, 8)).toEqual(
      Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    );
    expect(png.toString("latin1", 12, 16)).toBe("IHDR");
    expect(png.readUInt32BE(16)).toBe(1024);
    expect(png.readUInt32BE(20)).toBe(1024);
    // Colour type 2 is RGB with 8 bits a channel: no alpha, no palette.
    expect(png[24]).toBe(8);
    expect(png[25]).toBe(2);
    expect(png.includes(Buffer.from("tRNS"))).toBe(false);
  });
});
