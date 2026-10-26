import { describe, expect, it } from "vitest";

import { decodeSubscribeItem } from "@/lib/protocol/envelope";
import { demoScript, scenarios } from "./scenarios";

/** The kinds of the items of `items` that the decoder skips or refuses. */
function unread(items: readonly { readonly item: unknown }[]): string[] {
  const found: string[] = [];
  for (const { item } of items) {
    const decoded = decodeSubscribeItem(item);
    if (!decoded.ok) {
      found.push(`refused: ${decoded.reason}`);
    } else if (decoded.value.kind === "event" && decoded.value.event.kind === "other") {
      found.push(decoded.value.event.wireKind);
    } else if (decoded.value.kind === "draft" && decoded.value.draft.kind === "other") {
      found.push(decoded.value.draft.wireKind);
    }
  }
  return found;
}

describe("the scripted streams of the replay tests", () => {
  it("send only items that the decoder of the app reads", () => {
    for (const scenario of scenarios()) {
      expect({ name: scenario.name, unread: unread(scenario.items) }).toEqual({
        name: scenario.name,
        unread: [],
      });
    }
    expect(unread(demoScript().build("demo").items)).toEqual([]);
  });
});
