import { afterEach, describe, expect, test } from "vitest";
import { fakeError } from "../test-helpers";
import { errorMessage } from "./meeting-errors";
import { setLanguage } from "./i18n";

afterEach(() => setLanguage("en"));

describe("errorMessage", () => {
  test("is the text of the key with the detail and the command filled in", () => {
    expect(errorMessage(fakeError({ key: "meetings.error.summary_failed", detail: "API error 401" }))).toBe(
      "Summary failed: API error 401",
    );
    expect(
      errorMessage(fakeError({ kind: "MissingTool", key: "meetings.error.ytdlp_missing", detail: null, command: "brew install yt-dlp ffmpeg" })),
    ).toBe("yt-dlp isn't installed. Run: brew install yt-dlp ffmpeg");
  });

  test("an error that is all detail shows the detail as it is", () => {
    expect(errorMessage(fakeError({ key: "meetings.error.other", detail: "The recording was not found." }))).toBe(
      "The recording was not found.",
    );
  });

  test("a colon that leads to a detail there is none of is cut off", () => {
    expect(errorMessage(fakeError({ key: "meetings.error.summary_failed", detail: null }))).toBe("Summary failed");
    expect(errorMessage(fakeError({ key: "meetings.error.needs_ffmpeg", detail: null, command: null }))).toBe(
      "This format needs ffmpeg",
    );
  });

  test("speaks the language of the window", () => {
    setLanguage("vi");
    expect(errorMessage(fakeError({ key: "meetings.error.no_audio", detail: null }))).toBe("File không có âm thanh.");
  });

  test("a key that has no text shows its detail, and with no detail the general sentence", () => {
    expect(errorMessage(fakeError({ key: "meetings.error.from_the_future", detail: "Something new." }))).toBe("Something new.");
    expect(errorMessage(fakeError({ key: "meetings.error.from_the_future", detail: null }))).toBe("Something went wrong.");
  });

  test("an error that says nothing says that something went wrong", () => {
    expect(errorMessage(fakeError({ key: "meetings.error.other", detail: "" }))).toBe("Something went wrong.");
    expect(errorMessage(fakeError({ key: "meetings.error.other", detail: null }))).toBe("Something went wrong.");
  });
});
