// Joins uniwind classes (decision D1). Variants of the components are maps from a state
// to a class string, so two classes of one map never set the same property, and a plain
// join is enough: no clsx and no tailwind-merge.

/** A part of a class list. `false`, `null`, `undefined` and "" add nothing. */
export type ClassPart = string | false | null | undefined;

/** The classes of `parts` that are set, joined with one space. */
export function cn(...parts: readonly ClassPart[]): string {
  let classes = "";
  for (const part of parts) {
    if (typeof part === "string" && part !== "") {
      classes = classes === "" ? part : `${classes} ${part}`;
    }
  }
  return classes;
}
