// What the meetings window says of an error. The Rust side sends a key of the texts with what is behind it (`ErrorView`);
// the words are the window's, in its language.
import type { ErrorView } from "./api";
import { t } from "./i18n";

/**
 * The error as a sentence: the text of its key with `{detail}` and `{command}` filled in. A colon that was to lead to a
 * detail there is none of ("Summary failed: ") is cut off. A key with no text shows the detail, and an error that says
 * nothing says that something went wrong.
 */
export function errorMessage(error: ErrorView): string {
  const text = t(error.key, { detail: error.detail ?? "", command: error.command ?? "" });
  const filled = text === error.key ? (error.detail ?? "") : text;
  const trimmed = filled.replace(/:\s*$/, "").trim();
  return trimmed || t("meetings.error.generic");
}
