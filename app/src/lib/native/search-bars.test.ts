// The ref of a native search bar follows the bar: the lock blurs a bar while it is
// mounted, and no bar after it unmounts.

import { describe, expect, it, vi, type Mock } from "vitest";
import type { SearchBarCommands } from "react-native-screens";

import { blurSearchBars, searchBarRef } from "./search-bars";

interface FakeBar extends SearchBarCommands {
  readonly blur: Mock<() => void>;
}

function fakeBar(): FakeBar {
  return {
    focus: vi.fn<() => void>(),
    blur: vi.fn<() => void>(),
    clearText: vi.fn<() => void>(),
    toggleCancelButton: vi.fn<(show: boolean) => void>(),
    setText: vi.fn<(text: string) => void>(),
    cancelSearch: vi.fn<() => void>(),
  };
}

describe("searchBarRef", () => {
  it("blurs a mounted bar, and no bar after it unmounts", () => {
    const ref = searchBarRef();
    const bar = fakeBar();
    ref.current = bar;
    expect(ref.current).toBe(bar);
    blurSearchBars();
    expect(bar.blur).toHaveBeenCalledTimes(1);

    ref.current = null;
    blurSearchBars();
    expect(bar.blur).toHaveBeenCalledTimes(1);
  });

  it("keeps only the newest commands when the bar sets its ref again", () => {
    const ref = searchBarRef();
    const first = fakeBar();
    const second = fakeBar();
    ref.current = first;
    ref.current = second;
    blurSearchBars();
    expect(first.blur).not.toHaveBeenCalled();
    expect(second.blur).toHaveBeenCalledTimes(1);
    ref.current = null;
  });

  it("blurs the bars of two screens", () => {
    const one = searchBarRef();
    const two = searchBarRef();
    const a = fakeBar();
    const b = fakeBar();
    one.current = a;
    two.current = b;
    blurSearchBars();
    expect(a.blur).toHaveBeenCalledTimes(1);
    expect(b.blur).toHaveBeenCalledTimes(1);
    one.current = null;
    two.current = null;
  });
});
