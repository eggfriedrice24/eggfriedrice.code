// An SF Symbol in our own views (expo-symbols): chevrons, arrow.up, stop.fill, the gear,
// doc.on.doc and the like. The design gives each icon a size in points and a colour role.
// The tint is a native prop, so it reads the role from theme/roles.ts.

import { SymbolView, type SymbolViewProps, type SymbolWeight } from "expo-symbols";

import { roles, type Role } from "@/theme/roles";

/** The name of an SF Symbol. */
export type SymbolName = Extract<SymbolViewProps["name"], string>;

export interface SymbolIconProps {
  readonly name: SymbolName;
  /** The size in points, as the design gives it (for example 20 for header icons). */
  readonly size: number;
  /** The colour role of the glyph. */
  readonly tone: Role;
  /** The weight. The design draws thick strokes for chevrons, so they use semibold. */
  readonly weight?: SymbolWeight;
  /**
   * The VoiceOver label. Without it the icon is decoration and VoiceOver skips it: an
   * icon inside a button takes the label of the button.
   */
  readonly label?: string;
}

/** An SF Symbol with a size and a colour role. */
export function SymbolIcon(props: SymbolIconProps) {
  const label = props.label;
  return (
    <SymbolView
      name={props.name}
      size={props.size}
      tintColor={roles[props.tone]}
      weight={props.weight ?? "regular"}
      accessible={label !== undefined}
      accessibilityLabel={label}
      accessibilityRole={label === undefined ? undefined : "image"}
      accessibilityElementsHidden={label === undefined}
    />
  );
}
