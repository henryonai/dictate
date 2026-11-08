// The Whisper models list: what is on disk, and how each download is going.
import { api, errorText } from "./api";
import type { DownloadEvent, ModelView } from "./api";

/** Where a model's download stands. A model with no download going on, finished or not, is `idle`. */
export type Download =
  | { state: "idle" }
  | { state: "running" | "cancelling"; done: number; total: number | null }
  | { state: "failed"; error: string };

export class ModelList {
  rows = $state.raw<ModelView[]>([]);
  diskUsageMb = $state(0);
  /** What went wrong last, as the Rust side said it. */
  error = $state("");

  private downloads = $state<Record<string, Download>>({});
  private stopListening: (() => void) | undefined;
  private stopped = false;

  stateOf(id: string): Download {
    return this.downloads[id] ?? { state: "idle" };
  }

  /** Starts listening to the download events and loads the list. */
  async start(): Promise<void> {
    const stop = await api.onModelDownload((event) => this.hear(event));
    if (this.stopped) {
      stop();
    } else {
      this.stopListening = stop;
    }
    await this.refresh();
  }

  stop(): void {
    this.stopped = true;
    this.stopListening?.();
    this.stopListening = undefined;
  }

  async refresh(): Promise<void> {
    try {
      const [rows, usage] = await Promise.all([api.listModels(), api.modelsDiskUsageMb()]);
      this.rows = rows;
      this.diskUsageMb = usage;
    } catch (e) {
      this.error = errorText(e);
    }
  }

  async download(id: string): Promise<void> {
    this.error = "";
    this.downloads[id] = { state: "running", done: 0, total: null };
    try {
      await api.downloadModel(id);
      // The last event comes first as a rule. If it didn't, the row must not stay stuck: the disk tells the truth.
      const now = this.stateOf(id).state;
      if (now === "running" || now === "cancelling") {
        this.downloads[id] = { state: "idle" };
        await this.refresh();
      }
    } catch (e) {
      this.downloads[id] = { state: "failed", error: errorText(e) };
    }
  }

  /** Asks the download to stop. The row shows "Cancelling…" until the `cancelled` event says it has. */
  async cancel(id: string): Promise<void> {
    const now = this.stateOf(id);
    if (now.state !== "running") return;
    this.downloads[id] = { state: "cancelling", done: now.done, total: now.total };
    try {
      await api.cancelDownload(id);
    } catch (e) {
      this.error = errorText(e);
    }
  }

  async remove(id: string): Promise<void> {
    this.error = "";
    try {
      await api.deleteModel(id);
    } catch (e) {
      this.error = errorText(e);
    }
    await this.refresh();
  }

  private hear(event: DownloadEvent): void {
    const { id } = event;
    switch (event.state) {
      case "running":
        // Progress that was already on its way doesn't undo a Cancel.
        if (this.stateOf(id).state !== "cancelling") {
          this.downloads[id] = { state: "running", done: event.done, total: event.total };
        }
        break;
      case "failed":
        this.downloads[id] = { state: "failed", error: event.error ?? "" };
        void this.refresh();
        break;
      case "done":
      case "cancelled":
        this.downloads[id] = { state: "idle" };
        void this.refresh();
        break;
    }
  }
}
