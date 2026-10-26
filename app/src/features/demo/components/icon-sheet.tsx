// The Icon board: the app icon of direction b, "the yolk" (assets/icon), large and at the
// sizes of the device check, 60 pt and 29 pt.

import { Image } from "react-native";

import { MonoText } from "@/components/ui/mono-text";
import { SheetCase, SheetScroll } from "./sheet-layout";

// Metro resolves the image by this path; an import would not type without a module
// declaration for PNG files.
const ICON: number = require("../../../../assets/icon/icon.png");

/** Each size with its class. The PNG has no corners: iOS draws them. */
const SIZES: readonly (readonly [string, string])[] = [
  ["size-240", "large"],
  ["size-60", "home screen, 60 pt"],
  ["size-29", "settings, 29 pt"],
];

/** The Icon board. */
export function IconSheet() {
  return (
    <SheetScroll>
      {SIZES.map(([size, name]) => (
        <SheetCase key={name} name={name}>
          <Image source={ICON} className={size} accessibilityLabel="efr" />
        </SheetCase>
      ))}
      <SheetCase name="layers">
        <MonoText>
          background surface_1, egg white text_bright, yolk accent_primary with the ❯ cut out. iOS
          draws the corners. The clear and tinted looks of iOS 26 need an Icon Composer build
          (device check).
        </MonoText>
      </SheetCase>
    </SheetScroll>
  );
}
