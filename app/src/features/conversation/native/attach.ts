// The native side of attach (ADR 0026): the three pickers of iOS and efr-files. A build
// without them gives null: the package of a picker throws when it loads with no native
// module, so this file loads them only when the build has them (lib/native/native-parts.ts).
//
// - Photo Library: the system photo picker, images only, several at a time. It asks for no
//   permission, and it keeps the format of the photo, because efr-files makes the JPEG.
// - Take Photo: the system camera. It asks for the camera once, with the text that the
//   pairing scan shares.
// - Choose File: the system document picker, several at a time, each copied to the
//   caches of the app.

import { NATIVE_PARTS } from "@/lib/native/native-parts";
import { EfrFiles } from "../../../../modules/efr-files";
import type { AttachNative, PickedFile } from "../model/attachments";

type ImagePicker = typeof import("expo-image-picker");
type DocumentPicker = typeof import("expo-document-picker");
type ImageAsset = import("expo-image-picker").ImagePickerAsset;
type ImagePickerOptions = import("expo-image-picker").ImagePickerOptions;

/** The last part of a file uri, as a name for a photo that has none. */
function lastPart(uri: string): string {
  return decodeURIComponent(uri.slice(uri.lastIndexOf("/") + 1));
}

/**
 * A picked photo, named `fallback` when it has no name. Its size comes with its JPEG from
 * efr-files.
 */
function photoOf(asset: ImageAsset, fallback: string): PickedFile {
  return { kind: "photo", uri: asset.uri, name: asset.fileName ?? fallback };
}

/**
 * The name of a photo of the camera, which has no file name: the uri ends in a UUID, so
 * the time names it, such as `Photo 14.05.33.jpg`.
 */
function cameraName(now: Date): string {
  return `Photo ${two(now.getHours())}.${two(now.getMinutes())}.${two(now.getSeconds())}.jpg`;
}

/** `value` with two digits. */
function two(value: number): string {
  return String(value).padStart(2, "0");
}

/** The attach effects of the app, or null in a build without them. */
export function nativeAttach(): AttachNative | null {
  const files = EfrFiles;
  if (!NATIVE_PARTS.attach || files === null) {
    return null;
  }
  const images: ImagePicker = require("expo-image-picker");
  const documents: DocumentPicker = require("expo-document-picker");
  const photoOptions: ImagePickerOptions = {
    mediaTypes: "images",
    base64: false,
    exif: false,
    preferredAssetRepresentationMode: images.UIImagePickerPreferredAssetRepresentationMode.Current,
  };
  // The first `limit` assets. Each picked asset is a copy in the caches, so the copies
  // over the room of the draft go at once.
  const within = <T extends { readonly uri: string }>(assets: readonly T[], limit: number) => {
    for (const extra of assets.slice(limit)) {
      void files.remove(extra.uri).catch(() => undefined);
    }
    return assets.slice(0, limit);
  };
  return {
    files,
    async pick(source, limit) {
      if (source === "files") {
        const result = await documents.getDocumentAsync({
          multiple: true,
          copyToCacheDirectory: true,
        });
        if (result.canceled) {
          return [];
        }
        return within(result.assets, limit).map((asset) => {
          const pdf = asset.mimeType === "application/pdf" || /\.pdf$/i.test(asset.name);
          const file: { -readonly [K in keyof PickedFile]: PickedFile[K] } = {
            kind: pdf ? "pdf" : "file",
            uri: asset.uri,
            name: asset.name,
          };
          if (asset.size !== undefined) {
            file.bytes = asset.size;
          }
          return file;
        });
      }
      if (source === "camera") {
        const permission = await images.requestCameraPermissionsAsync();
        if (!permission.granted) {
          return [];
        }
        const result = await images.launchCameraAsync(photoOptions);
        const name = cameraName(new Date());
        return result.canceled ? [] : result.assets.map((asset) => photoOf(asset, name));
      }
      const result = await images.launchImageLibraryAsync({
        ...photoOptions,
        allowsMultipleSelection: true,
        selectionLimit: limit,
      });
      const picked = result.canceled ? [] : within(result.assets, limit);
      return picked.map((asset) => photoOf(asset, lastPart(asset.uri)));
    },
  };
}
