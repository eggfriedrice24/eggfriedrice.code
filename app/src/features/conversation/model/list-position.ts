// Where the reader is in the transcript list. The list is not inverted, so the newest
// row is at the end: the largest offset.

/** The part of a scroll event of React Native that the list reads. */
export interface ListScroll {
  readonly contentOffset: { readonly y: number };
  readonly contentSize: { readonly height: number };
  readonly layoutMeasurement: { readonly height: number };
  /** The inset under the content, such as the keyboard. iOS gives it. */
  readonly contentInset?: { readonly bottom: number };
}

/**
 * True when the reader is within one screen of the end. The end is the largest offset:
 * the content and the inset under it, less the height of the view.
 */
export function isNearEnd(scroll: ListScroll): boolean {
  const { contentOffset, contentSize, layoutMeasurement } = scroll;
  const inset = scroll.contentInset?.bottom ?? 0;
  const end = Math.max(0, contentSize.height + inset - layoutMeasurement.height);
  return end - contentOffset.y <= layoutMeasurement.height;
}
