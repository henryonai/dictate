// What the meetings window knows: the list of recordings, which one is selected and its detail, the jobs that are running,
// and the short notice of the last thing that failed.
//
// The list is the Rust side's. The jobs are heard as they go (`meeting-job`) and laid on the rows without asking for the
// list again: a search reads every summary and transcript of the library, so it is not asked for at every figure of
// progress. A change of the library (`meetings-changed`: a start, an end, a new title, a delete) brings the list again and
// forgets what was heard, which the new list says better.
import { SvelteMap } from "svelte/reactivity";
import { api, errorText } from "./api";
import type { ErrorView, JobView, MeetingDetail, MeetingRow, Readiness, SettingsTab } from "./api";
import { t } from "./i18n";
import { asErrorView, errorMessage } from "./meeting-errors";

/** How long the search waits for another key before it asks for the list. */
export const SEARCH_DELAY_MS = 200;

/** A question asked about a recording. `answer` is null while the model works on it. */
export interface QA {
  question: string;
  answer: string | null;
  error: ErrorView | null;
}

/** The row, with the state of the job that was heard of it laid on what the list said. */
function withJob(row: MeetingRow, job: JobView | undefined): MeetingRow {
  if (!job) return row;
  if (job.outcome === null) return { ...row, live: job, error: null };
  return { ...row, live: null, error: job.outcome.kind === "failed" ? job.outcome.error : null };
}

export class MeetingsStore {
  /** As the Rust side listed them for `query`: newest first. */
  rows = $state.raw<MeetingRow[]>([]);
  query = $state("");
  selectedId = $state<string | null>(null);
  /** The detail of the selected recording; null before it is read, and while another one is being read. */
  detail = $state.raw<MeetingDetail | null>(null);
  /** What the settings say is ready; null until it is read. */
  readiness = $state.raw<Readiness | null>(null);
  /** True once the list has been read: before that, an empty list is not yet an empty library. */
  loaded = $state(false);
  /** What went wrong last, in a sentence; empty when all is well. */
  notice = $state("");
  /** The recording whose log is open. */
  logFor = $state<string | null>(null);
  sidebarOpen = $state(true);
  /** The questions asked of each recording, with the answers: in memory only, lost when the window closes. */
  asks = new SvelteMap<string, QA[]>();

  /** The newest job state heard of each recording, until the next change of the library. */
  private jobs = $state.raw<Record<string, JobView>>({});
  private loads = 0;
  private detailLoads = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private unlisten: (() => void)[] = [];
  private stopped = false;
  private readonly onFocus = () => void this.refreshReadiness();

  /** The rows with the jobs that were heard laid on them. */
  items = $derived.by(() => this.rows.map((row) => withJob(row, this.jobs[row.id])));

  /** The row of the selected recording: the list's, or while the list doesn't have it yet, the detail's. */
  selectedRow = $derived.by(() => {
    const id = this.selectedId;
    if (id === null) return null;
    const listed = this.items.find((row) => row.id === id);
    if (listed) return listed;
    return this.detail?.row.id === id ? withJob(this.detail.row, this.jobs[id]) : null;
  });

  /** Starts listening to the jobs and to the library, and reads the list and what is ready. */
  async start(): Promise<void> {
    this.stopped = false;
    const stops = await Promise.all([
      api.onMeetingJob((job) => this.hearJob(job)),
      api.onMeetingsChanged(() => void this.refreshAll()),
    ]);
    if (this.stopped) {
      stops.forEach((stop) => stop());
      return;
    }
    this.unlisten = stops;
    // The keys are set in the Settings window: what is ready may be different when this one gets the focus back.
    window.addEventListener("focus", this.onFocus);
    await Promise.all([this.load(), this.refreshReadiness()]);
  }

  stop(): void {
    this.stopped = true;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.unlisten.forEach((stop) => stop());
    this.unlisten = [];
    window.removeEventListener("focus", this.onFocus);
  }

  async load(): Promise<void> {
    // Answers can come in another order than the questions: only the newest one counts.
    const mine = ++this.loads;
    let rows: MeetingRow[];
    try {
      rows = await api.meetingsList(this.query);
    } catch (e) {
      if (mine === this.loads) this.notify(errorText(e));
      return;
    }
    if (mine !== this.loads) return;
    this.rows = rows;
    this.loaded = true;
    // The recording that was selected is gone (deleted): the first one takes its place. A search may leave the selected
    // recording out of the list, which is not a reason to leave it.
    const id = this.selectedId;
    if (id !== null && this.query.trim() === "" && !rows.some((row) => row.id === id)) {
      await this.select(rows[0]?.id ?? null);
    }
  }

  async refreshReadiness(): Promise<void> {
    try {
      this.readiness = await api.meetingsReadiness();
    } catch (e) {
      this.notify(errorText(e));
    }
  }

  /** Types in the search box: the box shows it at once, and the list is asked for 200 ms after the last key. */
  search(query: string): void {
    this.query = query;
    clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = undefined;
      void this.load();
    }, SEARCH_DELAY_MS);
  }

  async select(id: string | null): Promise<void> {
    this.selectedId = id;
    // The detail of the one before is not this one's.
    if (this.detail?.row.id !== id) this.detail = null;
    if (id === null) {
      this.detailLoads += 1;
      return;
    }
    await this.reloadDetail();
  }

  async reloadDetail(): Promise<void> {
    const id = this.selectedId;
    if (id === null) return;
    const mine = ++this.detailLoads;
    try {
      const detail = await api.meetingDetail(id);
      if (mine === this.detailLoads && this.selectedId === id) this.detail = detail;
    } catch (e) {
      if (mine === this.detailLoads && this.selectedId === id) {
        this.selectedId = null;
        this.detail = null;
        this.notify(errorText(e));
      }
    }
  }

  // ---- What a person asks for

  /** Opens the dialog for files and imports what is picked. */
  async pickFiles(): Promise<void> {
    try {
      const ids = await api.meetingsPickFiles();
      if (ids.length > 0) await this.select(ids[0]);
    } catch (e) {
      this.notify(errorText(e));
    }
  }

  /** Imports the files that were dropped on the window. */
  async importFiles(paths: string[]): Promise<void> {
    if (paths.length === 0) return;
    try {
      const ids = await api.meetingsImportFiles(paths);
      if (ids.length === 0) this.notify(t("meetings.drop.none"));
      else await this.select(ids[0]);
    } catch (e) {
      this.notify(errorText(e));
    }
  }

  /** Imports a link. The sentence of why it could not be, or null when it was. */
  async importLink(url: string): Promise<string | null> {
    try {
      await this.select(await api.meetingsImportLink(url.trim()));
      return null;
    } catch (e) {
      return errorMessage(asErrorView(e));
    }
  }

  /** "Try again": from the step that failed. False when it could not start (the notice says why). */
  retry(id: string): Promise<boolean> {
    return this.restart(() => api.meetingRetry(id));
  }

  /** "Retry summary". */
  resummarize(id: string): Promise<boolean> {
    return this.restart(() => api.meetingResummarize(id));
  }

  /** "Transcribe + summarize again from scratch". */
  rerun(id: string): Promise<boolean> {
    return this.restart(() => api.meetingRerun(id));
  }

  async cancel(id: string): Promise<void> {
    try {
      await api.meetingCancel(id);
    } catch (e) {
      this.notify(errorText(e));
    }
  }

  /** Puts the text on the clipboard of the system. */
  async copy(text: string): Promise<boolean> {
    try {
      await api.copyText(text);
      return true;
    } catch (e) {
      this.notify(errorText(e));
      return false;
    }
  }

  async openSettings(tab: SettingsTab): Promise<void> {
    try {
      await api.meetingsOpenSettings(tab);
    } catch (e) {
      this.notify(errorText(e));
    }
  }

  notify(message: string): void {
    this.notice = message;
  }

  dismissNotice(): void {
    this.notice = "";
  }

  showLog(id: string): void {
    this.logFor = id;
  }

  closeLog(): void {
    this.logFor = null;
  }

  toggleSidebar(): void {
    this.sidebarOpen = !this.sidebarOpen;
  }

  // ---- What is heard

  private async restart(call: () => Promise<void>): Promise<boolean> {
    try {
      await call();
      return true;
    } catch (e) {
      // A recording that has a job says so (`meetings.error.busy`): a short notice, not a card with "Try again".
      this.notify(errorMessage(asErrorView(e)));
      return false;
    }
  }

  private hearJob(view: JobView): void {
    const before = this.jobs[view.id] ?? this.rows.find((row) => row.id === view.id)?.live ?? this.detail?.row.live;
    this.jobs = { ...this.jobs, [view.id]: view };
    if (view.id !== this.selectedId) return;
    // The transcript is written between two steps and no change of the library says so: a new step (or the end) is the
    // moment to read the detail again.
    if (view.outcome !== null || before?.step !== view.step) void this.reloadDetail();
  }

  private async refreshAll(): Promise<void> {
    this.jobs = {};
    const before = this.selectedId;
    await this.load();
    // `load` has read the detail of the recording that took the place of one that is gone.
    if (before !== null && this.selectedId === before) await this.reloadDetail();
  }
}
