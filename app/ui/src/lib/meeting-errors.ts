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

function isErrorView(value: unknown): value is ErrorView {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as ErrorView).kind === "string" &&
    typeof (value as ErrorView).key === "string"
  );
}

function sentenceOf(failure: unknown): string {
  if (typeof failure === "string") return failure;
  if (failure instanceof Error) return failure.message;
  if (typeof failure === "object" && failure !== null && typeof (failure as { message?: unknown }).message === "string") {
    return (failure as { message: string }).message;
  }
  return "";
}

/**
 * What a command failed with, as an error to show. Some commands reject with an `ErrorView` and the rest with a sentence in
 * English (or the IPC itself fails with an `Error`): that sentence is the detail of an error of no kind in particular.
 */
export function asErrorView(failure: unknown): ErrorView {
  if (isErrorView(failure)) return failure;
  return { kind: "Other", key: "meetings.error.other", detail: sentenceOf(failure), command: null };
}
