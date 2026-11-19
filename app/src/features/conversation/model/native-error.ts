// The code of an error of a local native module, such as `ERR_FILE_GONE`. Expo puts the
// code on the error that a function of the module rejects with. Pure TypeScript.

/** The `code` of `error`, or undefined when it has none. */
export function nativeErrorCode(error: unknown): unknown {
  return typeof error === "object" && error !== null && "code" in error ? error.code : undefined;
}
