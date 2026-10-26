import { describe, expect, it } from "vitest";

import { linkAction } from "./links";

describe("linkAction", () => {
  it("asks before it opens a web or mail address", () => {
    expect(linkAction("https://docs.rs/tokio")).toEqual({
      kind: "confirm",
      url: "https://docs.rs/tokio",
      host: "docs.rs",
    });
    expect(linkAction("HTTP://user@example.com:8080/x")).toMatchObject({
      kind: "confirm",
      host: "example.com:8080",
    });
    expect(linkAction("mailto:me@example.com")).toMatchObject({ kind: "confirm", host: "" });
  });

  it("refuses every other scheme and malformed addresses", () => {
    for (const url of [
      "javascript:alert(1)",
      "efr://pair?t=x",
      "file:///etc/passwd",
      "tel:123",
      "sms:123",
      "data:text/html,x",
      "/relative/path",
      "https://",
      "https://a b.com",
      "https://a.com/\u0000",
      "https://evil.example\\@good.example/",
      "https://evil.example\\.good.example/",
      "https://\u202eelpmaxe.lived/",
      "https://good.example\u2060.evil.example/",
    ]) {
      expect({ url, kind: linkAction(url).kind }).toEqual({ url, kind: "refuse" });
    }
  });
});
