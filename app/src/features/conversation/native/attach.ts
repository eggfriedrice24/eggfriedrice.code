// The native side of attach (spec 7.1): the three pickers of iOS and efr-files. A build
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

/** A picked photo. Its size comes with its JPEG from efr-files. */
function photoOf(asset: ImageAsset): PickedFile {
  return { kind: "photo", uri: asset.uri, name: asset.fileName ?? lastPart(asset.uri) };
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
        return result.assets.slice(0, limit).map((asset) => {
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
        return result.canceled ? [] : result.assets.map(photoOf);
      }
      const result = await images.launchImageLibraryAsync({
        ...photoOptions,
        allowsMultipleSelection: true,
        selectionLimit: limit,
      });
      return result.canceled ? [] : result.assets.slice(0, limit).map(photoOf);
    },
  };
}
