// The name that the phone sends with `device.enroll`. efrd shows it at the terminal, and
// the user confirms the pairing by it, so efr-store refuses a name with a control
// character or a character that changes how the text around it reads
// (crates/efr-store/src/devices.rs, `name_is_valid`). The app cleans the name first, so
// that the enroll does not fail on it.

/** The most characters (code points) of a device name, as efr-store. */
export const DEVICE_NAME_MAX_CHARS = 64;

/** The name when the phone gives none that is usable. */
export const FALLBACK_DEVICE_NAME = "iPhone";

/**
 * `raw` without control characters and invisible format characters, with white space
 * runs made one space, trimmed, and cut to 64 characters. Gives the fallback name when
 * nothing is left.
 */
export function deviceName(raw: string | null | undefined): string {
  const kept = Array.from(raw ?? "")
    .map((char) => (/^\s$/u.test(char) ? " " : char))
    .filter((char) => !isRefused(char.codePointAt(0) ?? 0))
    .join("")
    .replace(/ {2,}/g, " ")
    .trim();
  const cut = Array.from(kept).slice(0, DEVICE_NAME_MAX_CHARS).join("").trim();
  return cut === "" ? FALLBACK_DEVICE_NAME : cut;
}

// NOTE: keep this list the same as `name_is_valid` in crates/efr-store/src/devices.rs.
/** The characters that efr-store refuses in a name: Rust's `char::is_control` and more. */
function isRefused(point: number): boolean {
  return (
    point <= 0x1f ||
    (point >= 0x7f && point <= 0x9f) ||
    point === 0x061c ||
    (point >= 0x200b && point <= 0x200f) ||
    (point >= 0x2028 && point <= 0x202e) ||
    (point >= 0x2060 && point <= 0x2069) ||
    point === 0xfeff ||
    // A lone surrogate is not text: efrd's JSON reader refuses it.
    (point >= 0xd800 && point <= 0xdfff)
  );
}
