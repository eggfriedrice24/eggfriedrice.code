// The `meta` of the queries of the app. This is the only `Register` declaration of
// TanStack Query in the app: a second declaration of `queryMeta` with another type is a
// type error, so add a member here instead.

/** The `meta` of a query. */
export interface AppQueryMeta {
  /**
   * True: the persister writes the result to the disk, so the bar and the sheet
   * show it before the machine connects. Only for small lists and status.
   */
  readonly persist?: boolean;
}

declare module "@tanstack/react-query" {
  interface Register {
    queryMeta: AppQueryMeta;
  }
}

/** True when a query with `meta` is one that the persister writes. */
export function isPersistedMeta(meta: AppQueryMeta | undefined): boolean {
  return meta?.persist === true;
}
