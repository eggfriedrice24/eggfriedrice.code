// The functions of the native module EfrFiles (ios/EfrFilesModule.swift). A uri is a
// `file://` uri in the caches or the temporary directory of the app, where the pickers of
// iOS put their copies; any other uri is refused. A file that is gone gives an error with
// the code FILE_GONE.

/**
 * The code of the error for a file that is gone, such as a copy that iOS purged from the
 * caches. Expo makes it from the name of the Swift class `FileGoneException`.
 */
export const FILE_GONE = "ERR_FILE_GONE";

/** What `prepareImage` wrote: the JPEG to upload and the preview of its tile. */
export interface PreparedImage {
  readonly uri: string;
  readonly width: number;
  readonly height: number;
  readonly bytes: number;
  /** A JPEG of at most 512 px, for the tile. */
  readonly previewUri: string;
}

/** The facts of one file. */
export interface FileInfo {
  readonly name: string;
  readonly bytes: number;
}

/** The native module, as `requireOptionalNativeModule("EfrFiles")` gives it. */
export interface EfrFilesNative {
  /**
   * Writes a JPEG with no metadata whose longer edge is at most `maxEdge` pixels, with the
   * JPEG `quality` (0 to 1), and its preview. A smaller image keeps its size.
   */
  prepareImage(uri: string, maxEdge: number, quality: number): Promise<PreparedImage>;
  fileInfo(uri: string): Promise<FileInfo>;
  /** At most `length` bytes (1 MiB at most) from `offset`, as standard base64. */
  readChunk(uri: string, offset: number, length: number): Promise<string>;
  /** Removes the file. A missing file is not an error. */
  remove(uri: string): Promise<void>;
  /** Shows the file full screen in Quick Look, with `title` at the top. */
  quickLook(uri: string, title: string): Promise<void>;
}
