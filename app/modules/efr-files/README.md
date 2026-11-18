# efr-files

A local Expo module for iOS. It gives the app the bytes of the files that you attach to a
prompt. The photo picker, the camera and the document picker of iOS copy each picked file
into the caches of the app. This module makes a photo small, reads a file in chunks for
the upload (`attachment.put`) and removes the copies.

## What the module does

- `prepareImage` reads the photo with ImageIO. It reads HEIC, JPEG, PNG and the other image
  formats of iOS. It writes a JPEG whose longer edge is at most `maxEdge` pixels, with the
  orientation of the photo applied. ImageIO makes the image from a smaller decode, so a
  48 MP photo is never in memory at full size. A smaller photo keeps its size.
- The JPEG holds the pixels and their colour profile only. It has no EXIF data, so no GPS
  position leaves the phone. efrd checks each file again with its own rules.
- `prepareImage` also writes the preview of the attachment tile beside the JPEG: at most
  512 px, JPEG quality 0.8. Both files are in `Library/Caches/efr-files`, and iOS can
  remove them when the phone needs space.
- Each uri must name a file in the caches or in the temporary directory of the app. The
  module refuses every other uri, so a bad uri cannot read or remove another file.
- The size of a file comes from `fileSizeKey`. That is not one of the file timestamp APIs,
  so the privacy manifest of the app needs no reason for it.

All bytes cross to JavaScript as standard base64 strings.

| Function                              | Result                                                          |
| ------------------------------------- | --------------------------------------------------------------- |
| `prepareImage(uri, maxEdge, quality)` | The uri, size and bytes of the JPEG, and the uri of its preview |
| `fileInfo(uri)`                       | The name and the size in bytes of the file                      |
| `readChunk(uri, offset, length)`      | At most `length` bytes (1 MiB at most) from `offset`, as base64 |
| `remove(uri)`                         | Removes the file; a missing file is not an error                |

A file that is gone gives an error with the code `ERR_FILE_GONE`.

## Checks on an EAS build

The module cannot build on Linux, so CI does not compile it. The Linux binary of
expo-modules-macros checks the macros, but not the types:

```sh
node_modules/.pnpm/node_modules/@expo/modules-macros-linux-x64/ExpoModulesMacros \
  expand --diagnostics-only modules/efr-files/ios/
```

Do these checks on the first EAS development build:

1. The build log shows the pod `EfrFiles` and no Swift error in `EfrFilesModule.swift`.
2. In the app, `requireOptionalNativeModule("EfrFiles")` is not `null`, so the plus shows.
3. Attach a HEIC photo of 48 MP. The JPEG is at most 2000 px, it has no GPS tag, and its
   preview is at most 512 px.
