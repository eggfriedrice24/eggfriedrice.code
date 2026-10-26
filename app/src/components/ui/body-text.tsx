// Text in the system font (SF Pro): prose, your messages, titles of rows and the
// sentences of pairing (design section 0).

import { Text, type TextProps } from "react-native";

import { cn } from "./cn";
import { TEXT_TONE_CLASS, TYPE_CLASS, type BodySize, type TextTone } from "./text-classes";

export interface BodyTextProps extends Omit<TextProps, "style"> {
  /** The style of the type scale. The default is prose, 14/21. */
  readonly size?: BodySize;
  /** The colour role. The default is text_primary. */
  readonly tone?: TextTone;
}

/** A text in the system font with a size of the type scale and a colour role. */
export function BodyText(props: BodyTextProps) {
  const { size = "body", tone = "text_primary", className, ...rest } = props;
  return <Text {...rest} className={cn(TYPE_CLASS[size], TEXT_TONE_CLASS[tone], className)} />;
}
