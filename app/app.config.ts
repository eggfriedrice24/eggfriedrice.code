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
  const variant = VARIANT_CONFIG[appVariant(process.env["APP_VARIANT"])];
  const ios: NonNullable<ExpoConfig["ios"]> = {
    ...base.ios,
    bundleIdentifier: variant.iosBundleIdentifier,
  };
  return { ...base, ios, name: variant.appName, scheme: variant.scheme, slug: base.slug ?? "efr" };
}
