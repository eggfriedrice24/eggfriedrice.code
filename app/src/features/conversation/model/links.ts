// The link rule of the transcript: the model writes the links, so the app never opens
// one on a tap. It shows the whole address and asks first, and it opens only web and
// mail addresses. Another scheme (a custom app scheme, `javascript:`, `file:`, `tel:`,
// `sms:` and the like) could start an action in another app, so the app does not open
// it.

/** What a tap on a link does. */
export type LinkAction =
  | {
      readonly kind: "confirm";
      /** The address to open after the user agrees. */
      readonly url: string;
      /** The host, for the title of the question; empty for a mail address. */
      readonly host: string;
    }
  | { readonly kind: "refuse"; readonly url: string; readonly reason: string };

const OPEN_SCHEMES = new Set(["http:", "https:", "mailto:"]);

/** The action for a tap on a link to `url`. */
export function linkAction(url: string): LinkAction {
  const trimmed = url.trim();
  const scheme = /^([a-z][a-z0-9+.-]*:)/i.exec(trimmed)?.[1]?.toLowerCase();
  if (scheme === undefined) {
    return { kind: "refuse", url: trimmed, reason: "The link has no web address." };
  }
  if (!OPEN_SCHEMES.has(scheme)) {
    return {
      kind: "refuse",
      url: trimmed,
      reason: `efr does not open ${scheme} links.`,
    };
  }
  if (hasSpaceOrControl(trimmed)) {
    return { kind: "refuse", url: trimmed, reason: "The link has spaces or control characters." };
  }
  if (scheme === "mailto:") {
    return { kind: "confirm", url: trimmed, host: "" };
  }
  const host = /^[a-z]+:\/\/([^/?#]*)/i.exec(trimmed)?.[1] ?? "";
  const bare = host.replace(/^.*@/, "");
  if (bare.length === 0) {
    return { kind: "refuse", url: trimmed, reason: "The link has no host." };
  }
  return { kind: "confirm", url: trimmed, host: bare };
}

function hasSpaceOrControl(text: string): boolean {
  for (let i = 0; i < text.length; i++) {
    const unit = text.charCodeAt(i);
    if (unit <= 0x20 || unit === 0x7f || /\s/.test(text.charAt(i))) {
      return true;
    }
  }
  return false;
}
