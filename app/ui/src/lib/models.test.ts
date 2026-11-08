import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { api } from "./api";
import type { ModelView } from "./api";
import { ModelList } from "./models.svelte";
import { stubApi } from "../test-helpers";

const tiny: ModelView = { id: "tiny", size_mb: 75, downloaded: false, recommended: false };

let backend: ReturnType<typeof stubApi>;

beforeEach(() => {
  backend = stubApi();
  vi.mocked(api.listModels).mockResolvedValue([tiny]);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

/** Only the clock is faked, so that a test decides how long ago something started. */
function freezeTime() {
  vi.useFakeTimers({ toFake: ["Date"] });
}

function passTime(ms: number) {
  vi.setSystemTime(Date.now() + ms);
}

function later<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function started(): Promise<ModelList> {
  const list = new ModelList();
  await list.start();
  return list;
}

describe("the list", () => {
  test("holds the models and the space they take", async () => {
    vi.mocked(api.modelsDiskUsageMb).mockResolvedValue(75);
    const list = await started();
    expect(list.rows).toEqual([tiny]);
    expect(list.diskUsageMb).toBe(75);
  });

  test("stops listening to the download events when it stops", async () => {
    const list = await started();
    list.stop();
    backend.emitDownload({ id: "tiny", done: 1, total: 2, state: "running" });
    expect(list.stateOf("tiny")).toEqual({ state: "idle" });
  });

  test("says why the models can't be listed", async () => {
    vi.mocked(api.listModels).mockRejectedValue("No data folder");
    const list = await started();
    expect(list.error).toBe("No data folder");
  });
});

describe("a download", () => {
  test("runs at once, follows the events and ends with the model on disk", async () => {
    const list = await started();
    const download = later<void>();
    vi.mocked(api.downloadModel).mockReturnValue(download.promise);

    const running = list.download("tiny");
    expect(api.downloadModel).toHaveBeenCalledWith("tiny");
    expect(list.stateOf("tiny")).toEqual({ state: "running", done: 0, total: null });

    backend.emitDownload({ id: "tiny", done: 10, total: 75, state: "running" });
    expect(list.stateOf("tiny")).toEqual({ state: "running", done: 10, total: 75 });

    vi.mocked(api.listModels).mockResolvedValue([{ ...tiny, downloaded: true }]);
    backend.emitDownload({ id: "tiny", done: 75, total: 75, state: "done" });
    download.resolve();
    await running;

    await vi.waitFor(() => expect(list.rows[0].downloaded).toBe(true));
    expect(list.stateOf("tiny")).toEqual({ state: "idle" });
  });

  test("keeps showing Cancelling until the cancelled event arrives", async () => {
    freezeTime();
    const list = await started();
    vi.mocked(api.downloadModel).mockReturnValue(later<void>().promise);
    void list.download("tiny");
    passTime(1000);

    await list.cancel("tiny");
    expect(api.cancelDownload).toHaveBeenCalledWith("tiny");
    expect(list.stateOf("tiny").state).toBe("cancelling");

    // Progress that was already on its way doesn't bring the progress bar back.
    backend.emitDownload({ id: "tiny", done: 20, total: 75, state: "running" });
    expect(list.stateOf("tiny").state).toBe("cancelling");

    backend.emitDownload({ id: "tiny", done: 20, total: 75, state: "cancelled" });
    expect(list.stateOf("tiny")).toEqual({ state: "idle" });
  });

  test("a failure shows its error, and trying again starts it over", async () => {
    const list = await started();
    const download = later<void>();
    vi.mocked(api.downloadModel).mockReturnValue(download.promise);
    const running = list.download("tiny");

    backend.emitDownload({ id: "tiny", done: 5, total: 75, state: "failed", error: "API error 503: busy" });
    download.reject("API error 503: busy");
    await running;
    expect(list.stateOf("tiny")).toEqual({ state: "failed", error: "API error 503: busy" });

    vi.mocked(api.downloadModel).mockReturnValue(later<void>().promise);
    void list.download("tiny");
    expect(list.stateOf("tiny").state).toBe("running");
  });

  test("a refusal with no event still shows as a failure", async () => {
    const list = await started();
    vi.mocked(api.downloadModel).mockRejectedValue("Already downloading");
    await list.download("tiny");
    expect(list.stateOf("tiny")).toEqual({ state: "failed", error: "Already downloading" });
  });

  test("never stays stuck when the command ends and no last event came", async () => {
    const list = await started();
    vi.mocked(api.listModels).mockClear();
    await list.download("tiny");
    expect(list.stateOf("tiny")).toEqual({ state: "idle" });
    expect(api.listModels).toHaveBeenCalled();
  });

  test("a second call while it runs does nothing", async () => {
    const list = await started();
    vi.mocked(api.downloadModel).mockReturnValue(later<void>().promise);

    void list.download("tiny");
    void list.download("tiny");

    expect(api.downloadModel).toHaveBeenCalledTimes(1);
  });

  test("a Cancel right after the start belongs to the double click that started it, and does nothing", async () => {
    freezeTime();
    const list = await started();
    vi.mocked(api.downloadModel).mockReturnValue(later<void>().promise);
    void list.download("tiny");

    await list.cancel("tiny");
    expect(api.cancelDownload).not.toHaveBeenCalled();
    expect(list.stateOf("tiny").state).toBe("running");

    passTime(1000);
    await list.cancel("tiny");
    expect(api.cancelDownload).toHaveBeenCalledWith("tiny");
    expect(list.stateOf("tiny").state).toBe("cancelling");
  });

  test("the late end of an old download leaves a newer download of the model alone", async () => {
    const list = await started();
    const old = later<void>();
    vi.mocked(api.downloadModel).mockReturnValueOnce(old.promise);
    const first = list.download("tiny");

    // The old one is over as far as the screen knows, and the person starts it again at once.
    backend.emitDownload({ id: "tiny", done: 5, total: 75, state: "cancelled" });
    vi.mocked(api.downloadModel).mockReturnValueOnce(later<void>().promise);
    void list.download("tiny");
    expect(list.stateOf("tiny").state).toBe("running");

    old.resolve();
    await first;
    expect(list.stateOf("tiny").state).toBe("running");
  });

  test("nor does the late failure of an old download", async () => {
    const list = await started();
    const old = later<void>();
    vi.mocked(api.downloadModel).mockReturnValueOnce(old.promise);
    const first = list.download("tiny");

    backend.emitDownload({ id: "tiny", done: 5, total: 75, state: "cancelled" });
    vi.mocked(api.downloadModel).mockReturnValueOnce(later<void>().promise);
    void list.download("tiny");

    old.reject("The connection was lost");
    await first;
    expect(list.stateOf("tiny").state).toBe("running");
  });

  test("events of one model leave the others alone", async () => {
    const list = await started();
    backend.emitDownload({ id: "base", done: 1, total: 2, state: "running" });
    expect(list.stateOf("base").state).toBe("running");
    expect(list.stateOf("tiny").state).toBe("idle");
  });
});

describe("refreshing", () => {
  test("an older answer that comes late doesn't replace a newer one", async () => {
    const list = await started();
    const slow = later<ModelView[]>();
    vi.mocked(api.listModels).mockReturnValueOnce(slow.promise);
    const older = list.refresh();

    vi.mocked(api.listModels).mockResolvedValueOnce([{ ...tiny, downloaded: true }]);
    await list.refresh();
    expect(list.rows[0].downloaded).toBe(true);

    slow.resolve([tiny]);
    await older;
    expect(list.rows[0].downloaded).toBe(true);
  });
});

describe("deleting", () => {
  test("removes the model and refreshes the list", async () => {
    const list = await started();
    vi.mocked(api.listModels).mockResolvedValue([tiny]);
    await list.remove("tiny");
    expect(api.deleteModel).toHaveBeenCalledWith("tiny");
    expect(api.listModels).toHaveBeenCalledTimes(2);
  });

  test("says why a model couldn't be deleted", async () => {
    const list = await started();
    vi.mocked(api.deleteModel).mockRejectedValue("Could not delete the model: busy");
    await list.remove("tiny");
    expect(list.error).toBe("Could not delete the model: busy");
  });
});
