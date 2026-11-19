// The bytes of the files that the phone attaches to a prompt. The pickers of iOS copy each
// picked file into the caches of the app. This module makes a photo small before the
// upload, reads a file in chunks for `attachment.put`, and removes the copies.
//
// - `prepareImage` reads the photo with ImageIO (HEIC, JPEG, PNG and the other formats of
//   iOS) and writes a JPEG whose longer edge is at most `maxEdge` pixels. ImageIO makes the
//   thumbnail from a smaller decode, so a 48 MP photo is never in memory at full size. The
//   JPEG holds only the pixels and their colour profile: no EXIF, so no GPS position
//   leaves the phone. It also writes the preview of the attachment tile, at most 512 px,
//   beside it. efrd checks each file again with its own rules.
// - Each uri must name a file in the caches or the temporary directory of the app, so a bad
//   uri cannot read or remove another file of the app.
// - The size comes from `fileSizeKey`, which is not one of the file timestamp APIs that
//   need a reason in the privacy manifest of the app.
// - `quickLook` shows a file full screen in the Quick Look of iOS, with its name as the
//   title (board S-Attach: a tap on a photo, and Quick Look in the menu of a tile).

import ExpoModulesCore
import Foundation
import ImageIO
import QuickLook
import UIKit
import UniformTypeIdentifiers

@ExpoModule("EfrFiles")
public class EfrFilesModule: Module {
  /// Writes a JPEG of the image at `uri` with a longer edge of at most `maxEdge` pixels and
  /// the JPEG `quality` (0 to 1), and the preview of its tile. A smaller image keeps its size.
  @JS(.concurrent)
  func prepareImage(uri: String, maxEdge: Int, quality: Double) async throws -> PreparedImage {
    guard maxEdge > 0, quality >= 0, quality <= 1 else {
      throw InvalidImageOptionsException()
    }
    let input = try AppFiles.checked(uri)
    guard let source = CGImageSourceCreateWithURL(input as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary)
    else {
      throw ImageReadException(input.lastPathComponent)
    }
    let directory = try AppFiles.preparedDirectory()
    let name = UUID().uuidString
    let prepared = directory.appendingPathComponent("\(name).jpg")
    let preview = directory.appendingPathComponent("\(name)-preview.jpg")
    do {
      let image = try Jpeg.thumbnail(of: source, maxEdge: maxEdge, name: input.lastPathComponent)
      try Jpeg.write(image, to: prepared, quality: quality)
      // The preview comes from the small JPEG just written, so the large original (a
      // 48 MP HEIC) is decoded only once.
      guard
        let written = CGImageSourceCreateWithURL(
          prepared as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary)
      else {
        throw ImageReadException(prepared.lastPathComponent)
      }
      let small = try Jpeg.thumbnail(
        of: written, maxEdge: min(maxEdge, Jpeg.previewEdge), name: input.lastPathComponent)
      try Jpeg.write(small, to: preview, quality: Jpeg.previewQuality)
      return PreparedImage(
        uri: prepared.absoluteString,
        width: image.width,
        height: image.height,
        bytes: try AppFiles.size(of: prepared),
        previewUri: preview.absoluteString
      )
    } catch {
      try? FileManager.default.removeItem(at: prepared)
      try? FileManager.default.removeItem(at: preview)
      throw error
    }
  }

  /// The name and the size in bytes of the file at `uri`.
  @JS(.concurrent)
  func fileInfo(uri: String) async throws -> FileInfo {
    let url = try AppFiles.checked(uri)
    return FileInfo(name: url.lastPathComponent, bytes: try AppFiles.size(of: url))
  }

  /// At most `length` bytes of the file at `uri` from `offset`, as standard base64. A read
  /// at or after the end gives an empty string.
  @JS(.concurrent)
  func readChunk(uri: String, offset: Int, length: Int) async throws -> String {
    guard offset >= 0, length > 0, length <= AppFiles.maxChunkBytes else {
      throw InvalidRangeException()
    }
    let url = try AppFiles.checked(uri)
    let handle: FileHandle
    do {
      handle = try FileHandle(forReadingFrom: url)
    } catch {
      throw AppFiles.failure(error, url)
    }
    defer {
      try? handle.close()
    }
    try handle.seek(toOffset: UInt64(offset))
    return (try handle.read(upToCount: length) ?? Data()).base64EncodedString()
  }

  /// Removes the file at `uri`. A missing file is not an error.
  @JS(.concurrent)
  func remove(uri: String) async throws {
    let url = try AppFiles.checked(uri)
    do {
      try FileManager.default.removeItem(at: url)
    } catch let error as CocoaError where AppFiles.missing(error) {
      return
    }
  }

  /// Shows the file at `uri` full screen in Quick Look, with `title` at the top. It resolves
  /// when Quick Look starts to show.
  @JS(.concurrent)
  func quickLook(uri: String, title: String) async throws {
    let url = try AppFiles.checked(uri)
    guard FileManager.default.fileExists(atPath: url.path) else {
      throw FileGoneException(url.lastPathComponent)
    }
    try await MainActor.run {
      guard let presenter = self.appContext?.utilities?.currentViewController() else {
        throw NoScreenException()
      }
      let controller = QuickLookController()
      let source = QuickLookSource(item: QuickLookItem(url: url, title: title))
      controller.source = source
      controller.dataSource = source
      presenter.present(controller, animated: true)
    }
  }
}

/// The one file of a Quick Look, with the name that the app gives it: the file on the disk
/// has a UUID as its name.
private final class QuickLookItem: NSObject, QLPreviewItem {
  let previewItemURL: URL?
  let previewItemTitle: String?

  init(url: URL, title: String) {
    previewItemURL = url
    previewItemTitle = title
  }
}

/// Quick Look of one file. It keeps its data source, because `dataSource` is a weak
/// reference.
private final class QuickLookController: QLPreviewController {
  var source: QuickLookSource?
}

/// The data source of a Quick Look of one file.
private final class QuickLookSource: NSObject, QLPreviewControllerDataSource {
  private let item: QuickLookItem

  init(item: QuickLookItem) {
    self.item = item
  }

  func numberOfPreviewItems(in controller: QLPreviewController) -> Int {
    return 1
  }

  func previewController(_ controller: QLPreviewController, previewItemAt index: Int)
    -> QLPreviewItem
  {
    return item
  }
}

/// What `prepareImage` wrote.
@Record
struct PreparedImage {
  /// The `file://` uri of the JPEG to upload.
  var uri: String
  var width: Int
  var height: Int
  var bytes: Int
  /// The `file://` uri of the preview of the tile, at most 512 px.
  var previewUri: String
}

/// The facts of one file.
@Record
struct FileInfo {
  var name: String
  var bytes: Int
}

/// The files that the module may touch: those in the caches and the temporary directory of
/// the app, where the pickers and `prepareImage` write.
private enum AppFiles {
  /// The largest chunk: the largest message of the phone socket of efrd is 1 MiB.
  static let maxChunkBytes = 1 << 20

  /// The file URL of `uri`, when it names a file in the caches or the temporary directory.
  static func checked(_ uri: String) throws -> URL {
    guard let url = URL(string: uri), url.isFileURL else {
      throw NotAppFileException(uri)
    }
    let standard = url.standardizedFileURL
    let name = standard.lastPathComponent
    // The file can be gone, so only its directory resolves. iOS names the same container
    // both as /var and as /private/var, and both sides of the test resolve the same way.
    let directory = standard.deletingLastPathComponent().resolvingSymlinksInPath()
    let file = directory.appendingPathComponent(name)
    let roots = [
      FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first,
      FileManager.default.temporaryDirectory,
    ]
    for case let root? in roots {
      let base = root.standardizedFileURL.resolvingSymlinksInPath().path
      if !name.isEmpty, name != "..", file.path.hasPrefix(base + "/") {
        return file
      }
    }
    throw NotAppFileException(uri)
  }

  /// The directory of the prepared images and their previews, in the caches of the app.
  static func preparedDirectory() throws -> URL {
    guard let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first else {
      throw NotAppFileException("the caches directory")
    }
    let directory = caches.appendingPathComponent("efr-files", isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    return directory
  }

  /// The size in bytes of the regular file at `url`.
  static func size(of url: URL) throws -> Int {
    let values: URLResourceValues
    do {
      values = try url.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey])
    } catch {
      throw failure(error, url)
    }
    guard values.isRegularFile == true, let size = values.fileSize else {
      throw NotAFileException(url.lastPathComponent)
    }
    return size
  }

  /// True when `error` says that the file is not there.
  static func missing(_ error: CocoaError) -> Bool {
    return error.code == .fileNoSuchFile || error.code == .fileReadNoSuchFile
  }

  /// `error` as the exception of a missing file when it is one.
  static func failure(_ error: Error, _ url: URL) -> Error {
    if let cocoa = error as? CocoaError, missing(cocoa) {
      return FileGoneException(url.lastPathComponent)
    }
    return error
  }
}

/// The JPEG files of `prepareImage`.
private enum Jpeg {
  /// The longer edge of the preview of a tile, in pixels.
  static let previewEdge = 512
  /// The JPEG quality of the preview.
  static let previewQuality = 0.8

  /// The image of `source` with the orientation applied and a longer edge of at most
  /// `maxEdge` pixels. ImageIO never makes it larger than the image.
  static func thumbnail(of source: CGImageSource, maxEdge: Int, name: String) throws -> CGImage {
    let options: [CFString: Any] = [
      // Not the small thumbnail that a photo can carry: the size must be `maxEdge`.
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,
      kCGImageSourceShouldCacheImmediately: true,
      kCGImageSourceThumbnailMaxPixelSize: maxEdge,
    ]
    guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else {
      throw ImageReadException(name)
    }
    return image
  }

  /// Writes `image` to `url` as a JPEG with no metadata.
  static func write(_ image: CGImage, to url: URL, quality: Double) throws {
    guard
      let destination = CGImageDestinationCreateWithURL(
        url as CFURL, UTType.jpeg.identifier as CFString, 1, nil)
    else {
      throw ImageWriteException()
    }
    CGImageDestinationAddImage(
      destination, image, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
    guard CGImageDestinationFinalize(destination) else {
      throw ImageWriteException()
    }
  }
}

private final class NotAppFileException: GenericException<String> {
  override var reason: String {
    "efr reads only the files in its caches and its temporary directory, not '\(param)'"
  }
}

// Expo makes the code `ERR_FILE_GONE` from this name, and the app reads that code
// (`FILE_GONE` of src/EfrFiles.types.ts). A new name breaks the app.
private final class FileGoneException: GenericException<String> {
  override var reason: String {
    "The file '\(param)' is not there any more"
  }
}

private final class NotAFileException: GenericException<String> {
  override var reason: String {
    "'\(param)' is not a file"
  }
}

private final class ImageReadException: GenericException<String> {
  override var reason: String {
    "iOS cannot read '\(param)' as an image"
  }
}

private final class NoScreenException: Exception {
  override var reason: String {
    "efr has no screen to show Quick Look on"
  }
}

private final class ImageWriteException: Exception {
  override var reason: String {
    "iOS did not write the JPEG of the image"
  }
}

private final class InvalidImageOptionsException: Exception {
  override var reason: String {
    "The longer edge must be above 0 and the quality from 0 to 1"
  }
}

private final class InvalidRangeException: Exception {
  override var reason: String {
    "A chunk starts at 0 or later and holds 1 byte to 1 MiB"
  }
}
