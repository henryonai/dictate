import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { render } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { stubMeetingsApi } from "../test-helpers";
import MeetingsApp from "./MeetingsApp.svelte";

afterEach(() => vi.restoreAllMocks());

const uiRoot = join(dirname(fileURLToPath(import.meta.url)), "../..");

test("the build has a page for the meetings window, which the Rust side opens as meetings.html", () => {
  const config = readFileSync(join(uiRoot, "vite.config.ts"), "utf8");
  expect(config).toMatch(/meetings:\s*fileURLToPath\(new URL\("\.\/meetings\.html", import\.meta\.url\)\)/);

  const page = readFileSync(join(uiRoot, "meetings.html"), "utf8");
  expect(page).toContain('<div id="app"></div>');
  expect(page).toContain('<script type="module" src="/src/meetings/main.ts"></script>');
});

test("the page mounts the app of the meetings window", () => {
  const main = readFileSync(join(uiRoot, "src/meetings/main.ts"), "utf8");
  expect(main).toContain('import MeetingsApp from "./MeetingsApp.svelte"');
  expect(main).toContain("mount(MeetingsApp");

  stubMeetingsApi();
  const { container } = render(MeetingsApp);
  expect(container.querySelector("main")).not.toBeNull();
});
