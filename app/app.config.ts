import type { ConfigContext, ExpoConfig } from "expo/config";
import { z } from "zod";

// The app variant picks the name, the bundle id and the scheme, so the dev, preview and
// TestFlight apps can sit side by side on one iPhone. EAS sets APP_VARIANT per profile in
// eas.json; a local `expo start` without it gets the development variant.
const appVariantSchema = z.enum(["development", "preview", "production"]);
type AppVariant = z.infer<typeof appVariantSchema>;

interface VariantConfig {
  readonly appName: string;
  readonly iosBundleIdentifier: string;
  readonly scheme: string;
}

const VARIANT_CONFIG: Record<AppVariant, VariantConfig> = {
  development: {
    appName: "efr Dev",
    iosBundleIdentifier: "dev.eggfriedrice.efr.dev",
    scheme: "efr-dev",
  },
  preview: {
    appName: "efr Preview",
    iosBundleIdentifier: "dev.eggfriedrice.efr.preview",
    scheme: "efr-preview",
  },
  production: {
    appName: "efr",
    iosBundleIdentifier: "dev.eggfriedrice.efr",
    scheme: "efr",
  },
};

function appVariant(value: string | undefined): AppVariant {
  const result = appVariantSchema.safeParse(
    value === undefined || value === "" ? "development" : value,
  );
  if (!result.success) {
    throw new Error(`APP_VARIANT must be development, preview or production, not "${value}"`);
  }
  return result.data;
}

export default function config({ config: base }: ConfigContext): ExpoConfig {
  const name = appVariant(process.env["APP_VARIANT"]);
  const variant = VARIANT_CONFIG[name];
  const ios: NonNullable<ExpoConfig["ios"]> = {
    ...base.ios,
    bundleIdentifier: variant.iosBundleIdentifier,
  };
  // The dev client loads its JavaScript from Metro over plain http. App Transport Security
  // lets NSAllowsLocalNetworking reach a LAN address, but not the 100.x address of a
  // tailnet, so the development variant allows every load (as bondid does). The preview
  // and production builds keep the strict rules: they talk to efrd only over ws://, which
  // ATS does not check.
  if (name === "development") {
    const infoPlist: Record<string, unknown> = { ...base.ios?.infoPlist };
    infoPlist["NSAppTransportSecurity"] = { NSAllowsArbitraryLoads: true };
    infoPlist["NSLocalNetworkUsageDescription"] =
      "efr Dev loads its code from the development server on your computer.";
    ios.infoPlist = infoPlist;
  }
  return { ...base, ios, name: variant.appName, scheme: variant.scheme, slug: base.slug ?? "efr" };
}
