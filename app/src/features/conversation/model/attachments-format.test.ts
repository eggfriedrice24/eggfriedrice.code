import { describe, expect, it } from "vitest";

import { utf8Length } from "@/lib/stream/utf8";
import {
  KEPT_EXTENSION_BYTES,
  NAME_MAX_BYTES,
  byteSize,
  captionLine,
  fitName,
  overLimitLine,
  tileLabel,
  uploadPercent,
  withheldLine,
  type TileText,
} from "./attachments-format";

describe("fitName", () => {
  // The table of `a_long_name_keeps_its_extension` in efr-protocol.
  it.each([
    ["a name that fits", "shot.png", "shot.png"],
    ["exactly the limit", "x".repeat(NAME_MAX_BYTES), "x".repeat(NAME_MAX_BYTES)],
    ["a long stem", `${"x".repeat(300)}.png`, `${"x".repeat(NAME_MAX_BYTES - 4)}.png`],
    ["four byte characters", `${"\u{1f600}".repeat(100)}.jpeg`, `${"\u{1f600}".repeat(58)}.jpeg`],
    ["no extension", "y".repeat(300), "y".repeat(NAME_MAX_BYTES)],
    [
      "a dot at the start is no extension",
      `.${"z".repeat(300)}`,
      `.${"z".repeat(NAME_MAX_BYTES - 1)}`,
    ],
    [
      "a long extension is cut as the stem",
      `a.${"e".repeat(300)}`,
      `a.${"e".repeat(NAME_MAX_BYTES - 2)}`,
    ],
  ])("cuts %s as efr-protocol does", (_case, name, want) => {
    const fit = fitName(name);
    expect(fit).toBe(want);
    expect(utf8Length(fit)).toBeLessThanOrEqual(NAME_MAX_BYTES);
  });

  it("keeps the longest extension that a cut keeps whole", () => {
    const extension = `.${"e".repeat(KEPT_EXTENSION_BYTES - 1)}`;
    const fit = fitName(`${"s".repeat(300)}${extension}`);
    expect(fit.endsWith(`s${extension}`)).toBe(true);
    expect(utf8Length(fit)).toBe(NAME_MAX_BYTES);
  });
});

describe("byteSize", () => {
  // The table of `a_byte_size_has_one_decimal_only_under_ten` in efr-stdx.
  it.each([
    [0, "0 B"],
    [999, "999 B"],
    [2_000, "2.0 KB"],
    [9_999, "9.9 KB"],
    [14_200, "14 KB"],
    [999_999, "999 KB"],
    [2_180_000, "2.1 MB"],
    [310_000_000, "310 MB"],
    [1_250_000_000, "1.2 GB"],
  ])("writes %d bytes as %s", (bytes, want) => {
    expect(byteSize(bytes)).toBe(want);
  });
});

const photo = (state: TileText["state"], bytes: number | undefined = 2_400_000): TileText => ({
  kind: "photo",
  name: "IMG_2044.jpg",
  bytes,
  state,
});

describe("the caption and the label of a tile", () => {
  it("puts the state in the place of the size", () => {
    expect(captionLine(photo({ kind: "size" }))).toBe("2.4 MB");
    expect(captionLine(photo({ kind: "preparing" }, undefined))).toBe("Preparing");
    expect(captionLine(photo({ kind: "uploading", percent: 40 }))).toBe("40%");
    expect(captionLine(photo({ kind: "waiting" }))).toBe("Waiting");
    expect(captionLine(photo({ kind: "again", percent: 10 }))).toBe("Uploading again");
    expect(captionLine(photo({ kind: "not_sent" }))).toBe("Not sent");
    expect(captionLine(photo({ kind: "as_path" }))).toBe("As a path");
    expect(captionLine(photo({ kind: "over_limit", limit: 20 * 1024 * 1024 }))).toBe("2.4 MB");
  });

  it("reads the kind, the name, the size and the state to VoiceOver (board S-Attach 9)", () => {
    expect(tileLabel(photo({ kind: "preparing" }, undefined))).toBe(
      "Photo, IMG_2044.jpg, preparing",
    );
    expect(tileLabel(photo({ kind: "uploading", percent: 40 }))).toBe(
      "Photo, IMG_2044.jpg, 2.4 MB, uploading 40 percent",
    );
    expect(tileLabel(photo({ kind: "waiting" }))).toBe(
      "Photo, IMG_2044.jpg, 2.4 MB, waiting to upload",
    );
    expect(tileLabel(photo({ kind: "again", percent: 10 }))).toBe(
      "Photo, IMG_2044.jpg, 2.4 MB, uploading again, 10 percent",
    );
    expect(tileLabel(photo({ kind: "not_sent" }))).toBe(
      "Photo, IMG_2044.jpg, 2.4 MB, not sent. The upload did not arrive.",
    );
    const zip: TileText = {
      kind: "file",
      name: "logs.zip",
      bytes: 34_000_000,
      state: { kind: "over_limit", limit: 20 * 1024 * 1024 },
    };
    expect(tileLabel(zip)).toBe("File, logs.zip, 34 MB. The limit is 20 MB.");
    const manual: TileText = {
      kind: "pdf",
      name: "manual.pdf",
      bytes: 18_000_000,
      state: { kind: "as_path" },
    };
    expect(tileLabel(manual)).toBe("PDF, manual.pdf, 18 MB, as a path");
  });

  it("counts the percent down and never over 100", () => {
    expect(uploadPercent(0, 0)).toBe(0);
    expect(uploadPercent(999, 1_000)).toBe(99);
    expect(uploadPercent(2_000, 1_000)).toBe(100);
  });
});

describe("the lines of the limits and of a withheld attachment", () => {
  it("names the file, its size and the limit", () => {
    expect(overLimitLine("logs.zip", 34_000_000, 20 * 1024 * 1024)).toBe(
      "logs.zip is 34 MB. The limit is 20 MB.",
    );
  });

  it("says why efrd sends the attachments as a path", () => {
    expect(withheldLine(["shot.png"], "refused")).toBe(
      "Not sent: shot.png. The provider refused it.",
    );
    expect(withheldLine(["a.png", "b.pdf"], "budget")).toBe(
      "Not sent: a.png, b.pdf. The request had no room for them.",
    );
    expect(withheldLine(["a.png"], "unknown")).toBe("Not sent: a.png.");
  });
});
