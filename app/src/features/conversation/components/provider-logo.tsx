// The logo of a model's account in the rows of the model sheet (S-ModelSheet): a 24 pt
// slot with the mark at 12 pt in its middle.
//
// - Claude models: the Claude spark in its own orange (#D97757). The colour is in the
//   image (an asset of the brand, not a colour of the UI), so the image takes no tint.
// - OpenAI models: the OpenAI mark, a black template image that `tintColor` paints in
//   text_primary.
// - A model of another provider has an empty slot, so the names stay in one column.
//
// The PNG images are in assets/providers, made from the provider icons of opencode.

import { Image, View } from "react-native";

import { cn } from "@/components/ui/cn";
import { roles } from "@/theme/roles";
import type { ProviderLogoKind } from "../model/accounts";

// Metro resolves the images by these paths; an import would not type without a module
// declaration for PNG files.
const CLAUDE: number = require("../../../../assets/providers/claude.png");
const OPENAI: number = require("../../../../assets/providers/openai.png");

export interface ProviderLogoProps {
  readonly logo: ProviderLogoKind;
  /** True for the row of an account that is signed out or full: the logo at 40 %. */
  readonly dim?: boolean;
}

/** The logo slot of a row. */
export function ProviderLogo(props: ProviderLogoProps) {
  return (
    <View
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
      className={cn("h-24 w-24 items-center justify-center", props.dim === true && "opacity-40")}
    >
      {props.logo === "claude" ? (
        <Image testID="provider-logo" source={CLAUDE} className="h-12 w-12" />
      ) : null}
      {props.logo === "openai" ? (
        <Image
          testID="provider-logo"
          source={OPENAI}
          tintColor={roles.text_primary}
          className="h-12 w-12"
        />
      ) : null}
    </View>
  );
}
