import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { render } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import { stubRecordingApi } from "../test-helpers";
import { api } from "../lib/api";
import RecorderApp from "./RecorderApp.svelte";

afterEach(() => vi.restoreAllMocks());

const uiRoot = join(dirname(fileURLToPath(import.meta.url)), "../..");

test("the build has a page for the recording bar, which the Rust side opens as recorder.html", () => {
  const config = readFileSync(join(uiRoot, "vite.config.ts"), "utf8");
  expect(config).toMatch(/recorder:\s*fileURLToPath\(new URL\("\.\/recorder\.html", import\.meta\.url\)\)/);

  const page = readFileSync(join(uiRoot, "recorder.html"), "utf8");
  expect(page).toContain('<div id="app"></div>');
  expect(page).toContain('<script type="module" src="/src/recorder/main.ts"></script>');
});

test("the page mounts the app of the bar, with the look of a transparent window and not the settings'", () => {
  const main = readFileSync(join(uiRoot, "src/recorder/main.ts"), "utf8");
  expect(main).toContain('import RecorderApp from "./RecorderApp.svelte"');
  expect(main).toContain("mount(RecorderApp");
  expect(main).toContain('import "./recorder.css"');
  expect(main).not.toContain("app.css");

  const css = readFileSync(join(uiRoot, "src/recorder/recorder.css"), "utf8");
  expect(css).toMatch(/html,\s*body\s*\{[^}]*background:\s*transparent/);

  stubRecordingApi();
  vi.spyOn(api, "panelLanguage").mockResolvedValue("en");
  const { container } = render(RecorderApp);
  expect(container.querySelector("main")).not.toBeNull();
});
