// Text in SF Mono: our own views use it for machine names, commands, paths, status and
// hints (design section 0). Native menus and sheets cannot use it (design note 3).

import { Text, type TextProps } from "react-native";

import { cn } from "./cn";
import { TEXT_TONE_CLASS, TYPE_CLASS, type MonoSize, type TextTone } from "./text-classes";

export interface MonoTextProps extends Omit<TextProps, "style"> {
  /** The style of the type scale. The default is the minimum, 11/16. */
  readonly size?: MonoSize;
  /** The colour role. The default is the quiet text_subtle of hints and status. */
  readonly tone?: TextTone;
}

/** A text in SF Mono with a size of the type scale and a colour role. */
export function MonoText(props: MonoTextProps) {
  const { size = "monoXs", tone = "text_subtle", className, ...rest } = props;
  return <Text {...rest} className={cn(TYPE_CLASS[size], TEXT_TONE_CLASS[tone], className)} />;
}
