import { describe, expect, it } from "vitest";

import { createRowUiStore, rowFlag } from "./row-ui-state";

describe("the row state", () => {
  it("opens and closes a flag of one row, and leaves the other rows alone", () => {
    const rows = createRowUiStore();
    const tool = rowFlag("t:t1:c", "expanded");
    const other = rowFlag("r:t1", "expanded");
    rows.toggle("t:t1:c", "expanded");
    expect(tool(rows.store.get())).toBe(true);
    expect(other(rows.store.get())).toBe(false);
    rows.set("t:t1:c", "all_hunks", true);
    rows.toggle("t:t1:c", "expanded");
    expect(tool(rows.store.get())).toBe(false);
    expect(rowFlag("t:t1:c", "all_hunks")(rows.store.get())).toBe(true);
  });

  it("keeps the state object when nothing changes, and drops a row with no flags", () => {
    const rows = createRowUiStore();
    const empty = rows.store.get();
    rows.set("m:t:0", "expanded", false);
    expect(rows.store.get()).toBe(empty);
    rows.set("m:t:0", "expanded", true);
    rows.set("m:t:0", "expanded", false);
    expect(rows.store.get().size).toBe(0);
  });
});
