import { act, cleanup, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { api } from "../lib/api";
import { setLanguage } from "../lib/i18n";
import { fakePanelState, stubPanelApi } from "../test-helpers";
import PanelApp from "./PanelApp.svelte";

let panel: ReturnType<typeof stubPanelApi>;

beforeEach(() => {
  panel = stubPanelApi();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  setLanguage("en");
});

/** The bubble window, once it listens. */
async function renderPanel(view: string) {
  const result = render(PanelApp, { view });
  await vi.waitFor(() => expect(api.onPanelState).toHaveBeenCalled());
  return result;
}

test("panel_ignores_unknown_view: a page opened with a view that is none of ours shows nothing and asks nothing", async () => {
  const { container } = render(PanelApp, { view: "settings" });
  await Promise.resolve();

  expect(container.children).toHaveLength(0);
  expect(api.onPanelState).not.toHaveBeenCalled();
  expect(api.panelResized).not.toHaveBeenCalled();
  expect(api.panelLanguage).not.toHaveBeenCalled();

  cleanup();
  const none = render(PanelApp, { view: null });
  expect(none.container.children).toHaveLength(0);
});

test("the bubble window shows the bubble for a state of the bubble view", async () => {
  await renderPanel("bubble");
  act(() => panel.emitState(fakePanelState({ view: "bubble", committed: "Hello" })));

  expect(await screen.findByText("Hello")).toBeInTheDocument();
  expect(screen.getByText("Listening")).toBeInTheDocument();
});

test("the bubble window shows nothing while the view is hidden", async () => {
  const { container } = await renderPanel("bubble");
  expect(container.querySelector(".bubble")).toBeNull();

  act(() => panel.emitState(fakePanelState({ view: "bubble" })));
  await screen.findByText("Listening");
  act(() => panel.emitState(fakePanelState({ view: "hidden", phase: "hidden" })));

  await vi.waitFor(() => expect(container.querySelector(".bubble")).toBeNull());
});

test("each window shows only its own views: the state goes to both, and the other one's is not drawn", async () => {
  const bubble = await renderPanel("bubble");
  act(() => panel.emitState(fakePanelState({ view: "pill", durationS: 3 })));
  await Promise.resolve();
  expect(bubble.container.querySelector(".bubble, .pill")).toBeNull();
  cleanup();

  const card = await renderPanel("card");
  act(() => panel.emitState(fakePanelState({ view: "bubble" })));
  await Promise.resolve();
  expect(card.container.querySelector(".bubble, .pill")).toBeNull();
});

test("the card window shows the pill for a state of the pill view", async () => {
  await renderPanel("card");
  act(() => panel.emitState(fakePanelState({ view: "pill", durationS: 65 })));

  expect(await screen.findByText("01:05")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
});

test("the words follow the language of the UI settings, asked on the start and each time the panel hides", async () => {
  vi.mocked(api.panelLanguage).mockResolvedValue("vi");
  await renderPanel("bubble");
  await vi.waitFor(() => expect(api.panelLanguage).toHaveBeenCalledOnce());

  act(() => panel.emitState(fakePanelState({ view: "bubble" })));
  expect(await screen.findByText("Đang nghe")).toBeInTheDocument();
  // A state that goes on showing doesn't ask again.
  act(() => panel.emitState(fakePanelState({ view: "bubble", level: 0.5 })));
  expect(api.panelLanguage).toHaveBeenCalledOnce();

  // The setting is changed in Settings; the panel hides, and has the change before it shows again.
  vi.mocked(api.panelLanguage).mockResolvedValue("en");
  act(() => panel.emitState(fakePanelState({ view: "hidden", phase: "hidden" })));
  await vi.waitFor(() => expect(api.panelLanguage).toHaveBeenCalledTimes(2));
  act(() => panel.emitState(fakePanelState({ view: "bubble" })));
  expect(await screen.findByText("Listening")).toBeInTheDocument();
});

test("the window stops listening when the page goes away", async () => {
  const { unmount } = await renderPanel("bubble");
  unmount();
  await vi.waitFor(() => expect(panel.stopListening).toHaveBeenCalledOnce());
});

test("the size of what is drawn is reported to the Rust side under the name of the window", async () => {
  type Callback = ResizeObserverCallback;
  let notify: Callback | undefined;
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: Callback) {
        notify = callback;
      }
      observe() {}
      disconnect() {}
      unobserve() {}
    },
  );
  const rect = { x: 0, y: 0, left: 0, top: 0, right: 427.3, bottom: 99.2, width: 427.3, height: 99.2, toJSON() {} };
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(rect);

  await renderPanel("bubble");
  act(() => panel.emitState(fakePanelState({ view: "bubble", committed: "Hello" })));
  await screen.findByText("Hello");
  notify?.([], {} as ResizeObserver);

  // In whole pixels, rounded up, so that the window is never a fraction too small.
  await vi.waitFor(() => expect(api.panelResized).toHaveBeenCalledWith("bubble", 428, 100));
  // The same size is not reported twice.
  notify?.([], {} as ResizeObserver);
  const calls = vi.mocked(api.panelResized).mock.calls.filter(([, w, h]) => w === 428 && h === 100);
  expect(calls).toHaveLength(1);
});
