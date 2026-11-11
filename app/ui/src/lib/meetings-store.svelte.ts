// What the meetings window knows: the list of recordings, which one is selected and its detail, the jobs that are running,
// and the short notice of the last thing that failed.
//
// The list is the Rust side's. The jobs are heard as they go (`meeting-job`) and laid on the rows without asking for the
// list again: a search reads every summary and transcript of the library, so it is not asked for at every figure of
// progress. A change of the library (`meetings-changed`: a start, an end, a new title, a delete) brings the list again and
// forgets what was heard, which the new list says better.
import { SvelteMap } from "svelte/reactivity";
import { api, asErrorView, errorText } from "./api";
import type { ErrorView, JobView, MeetingDetail, MeetingRow, Readiness, RecordingView, SettingsTab } from "./api";
import { setLanguage, t } from "./i18n";
import { errorMessage } from "./meeting-errors";
import { IDLE_RECORDING } from "./recording";

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
  /** The dialog of "Paste a link" is open. */
  linkDialogOpen = $state(false);
  /** How the recording goes: idle before the recorder has said, and when nothing is recorded. */
  recording = $state.raw<RecordingView>(IDLE_RECORDING);
  /**
   * The picker of the recording ("Record & summarize") is open while this is set. `error` is why a recording did not begin, when
   * the picker was asked for because of that (a start from the shortcut), and what a start from the picker said.
   */
  recorder = $state.raw<{ error: ErrorView | null } | null>(null);
  sidebarOpen = $state(true);
  /**
   * The rename of speakers that was asked for: the speaker whose name was clicked, or null for the sheet as a whole. The sheet is
   * open while this is set and clears it when it closes.
   */
  renameFor = $state<{ speaker: string | null } | null>(null);
  /** The questions asked of each recording, with the answers: in memory only, lost when the window closes. */
  asks = new SvelteMap<string, QA[]>();

  /** The newest job state heard of each recording, until the next change of the library. */
  private jobs = $state.raw<Record<string, JobView>>({});
  private loads = 0;
  private detailLoads = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private unlisten: (() => void)[] = [];
  private stopped = false;
  /** A state of the recording came by the event: the answer to the question asked at the start is older than it. */
  private heardRecording = false;
  private readonly onFocus = () => void this.refreshReadiness();

  /** A dialog is open over the window: what is pasted or dropped on the window is not for the library then. */
  modalOpen = $derived(this.logFor !== null || this.linkDialogOpen || this.renameFor !== null || this.recorder !== null);

  /** A recording is going on: it is being made, it is paused, or it is being put together. */
  isRecording = $derived(this.recording.phase !== "idle");

  /**
   * The rows with the jobs that were heard laid on them. The folder of a recording that goes on is in the library already, and
   * would show as a recording that did not finish: it is a row when the recording is over.
   */
  items = $derived.by(() => {
    const going = this.isRecording ? this.recording.meetingId : null;
    return this.rows.filter((row) => row.id !== going).map((row) => withJob(row, this.jobs[row.id]));
  });

  /** The row of the selected recording: the list's, or while the list doesn't have it yet, the detail's. */
  selectedRow = $derived.by(() => {
    const id = this.selectedId;
    if (id === null) return null;
    const listed = this.items.find((row) => row.id === id);
    if (listed) return listed;
    return this.detail?.row.id === id ? withJob(this.detail.row, this.jobs[id]) : null;
  });

  /**
   * Starts listening to the jobs and to the library, and reads the list and what is ready. A listener that can't be set up is
   * said in the notice, and the list is read all the same: the window works, only without that news.
   */
  async start(): Promise<void> {
    this.stopped = false;
    this.heardRecording = false;
    const settled = await Promise.allSettled([
      api.onMeetingJob((job) => this.hearJob(job)),
      api.onMeetingsChanged(() => void this.refreshAll()),
      api.onRecordingState((view) => {
        this.heardRecording = true;
        this.recording = view;
      }),
    ]);
    const stops = settled.flatMap((result) => (result.status === "fulfilled" ? [result.value] : []));
    if (this.stopped) {
      stops.forEach((stop) => stop());
      return;
    }
    this.unlisten = stops;
    const failed = settled.find((result) => result.status === "rejected");
    if (failed) this.notify(errorText(failed.reason));
    // The keys are set in the Settings window: what is ready may be different when this one gets the focus back.
    window.addEventListener("focus", this.onFocus);
    await Promise.all([this.load(), this.refreshReadiness(), this.refreshRecording()]);
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

  /**
   * Reads what is ready and the language of the UI, which the settings choose and this window can't read: it speaks it from now
   * on. It runs when the window opens and each time it gets the focus back, which is when a change in Settings can be seen.
   */
  async refreshReadiness(): Promise<void> {
    try {
      const readiness = await api.meetingsReadiness();
      this.readiness = readiness;
      setLanguage(readiness.language);
    } catch (e) {
      this.notify(errorText(e));
    }
  }

  /** Asks how the recording is, for a window that opens in the middle of one. A state that was heard meanwhile is newer than the answer. */
  async refreshRecording(): Promise<void> {
    try {
      const view = await api.recordingState();
      if (!this.heardRecording) this.recording = view;
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
    this.renameFor = null;
    // The detail of the one before is not this one's.
    if (this.detail?.row.id !== id) this.detail = null;
    if (id === null) {
      this.detailLoads += 1;
      return;
    }
    await this.reloadDetail();
  }

  /**
   * Reads the detail of the selected recording again. When it can't be read the recording is let go of; `silent` says nothing of
   * it, for a reload that nobody asked for (the library changed, a job moved on), where the recording being gone is no news.
   */
  async reloadDetail(silent = false): Promise<void> {
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
        if (!silent) this.notify(errorText(e));
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

  /** Asks a question about the recording. It is kept with the answer (or why there is none); one is asked at a time for a recording. */
  async ask(id: string, question: string): Promise<void> {
    const text = question.trim();
    if (text === "" || this.asking(id)) return;
    const entry: QA = { question: text, answer: null, error: null };
    this.asks.set(id, [...(this.asks.get(id) ?? []), entry]);
    let result: QA;
    try {
      result = { ...entry, answer: await api.meetingAsk(id, text) };
    } catch (e) {
      result = { ...entry, error: asErrorView(e) };
    }
    // A recording that was deleted meanwhile has no questions to come back to.
    const kept = this.asks.get(id);
    if (kept) this.asks.set(id, kept.map((qa) => (qa === entry ? result : qa)));
  }

  /** A question of the recording is waiting for its answer. */
  asking(id: string): boolean {
    return (this.asks.get(id) ?? []).some((qa) => qa.answer === null && qa.error === null);
  }

  /**
   * Gives speakers new names (`names`: old to new) in the transcript and the summary, and with `resummarize` has the summary written
   * again from the transcript with them. Null when it was done; else why not, as a view: the sheet that asked says it in itself and
   * stays open, so it is not a notice.
   */
  async renameSpeakers(id: string, names: Record<string, string>, resummarize: boolean): Promise<ErrorView | null> {
    try {
      await api.meetingRenameSpeakers(id, names, resummarize);
      return null;
    } catch (e) {
      return asErrorView(e);
    }
  }

  /** Ticks or unticks an action item (`index` is the one of its `ActionView`). False when it could not be written: the notice says why. */
  async setTask(id: string, index: number, done: boolean): Promise<boolean> {
    try {
      await api.meetingSetTask(id, index, done);
      return true;
    } catch (e) {
      this.notify(errorText(e));
      return false;
    }
  }

  async openFolder(id: string): Promise<void> {
    try {
      await api.meetingOpenFolder(id);
    } catch (e) {
      this.notify(errorText(e));
    }
  }

  /** The video or audio in the program of the system for it. */
  async openMedia(id: string): Promise<void> {
    try {
      await api.meetingOpenMedia(id);
    } catch (e) {
      this.notify(errorMessage(asErrorView(e)));
    }
  }

  /** Saves the Markdown in a file the person chooses. False when the dialog was closed, and when it could not be written (the notice says why). */
  async exportMarkdown(id: string): Promise<boolean> {
    try {
      return await api.meetingExport(id);
    } catch (e) {
      this.notify(errorMessage(asErrorView(e)));
      return false;
    }
  }

  /** Puts the whole Markdown of the recording (summary and transcript) on the clipboard. */
  async copyMarkdown(id: string): Promise<boolean> {
    let markdown: string;
    try {
      markdown = await api.meetingMarkdown(id);
    } catch (e) {
      this.notify(errorText(e));
      return false;
    }
    return this.copy(markdown);
  }

  /**
   * Moves the recording to the Trash. `release` is called first: the player lets go of the file, because Windows may refuse to move a
   * folder that has a file open; it gives back what puts the player as it was. Then the first row of the list is selected. When it fails
   * the notice says why, the recording stays, the player is put back and the recording is read again.
   */
  async remove(id: string, release?: () => (() => void) | void): Promise<boolean> {
    const restore = release?.();
    try {
      await api.meetingDelete(id);
    } catch (e) {
      this.notify(errorMessage(asErrorView(e)));
      restore?.();
      await this.reloadDetail(true);
      return false;
    }
    this.asks.delete(id);
    await this.load();
    // `load` has taken the first row of a whole list; the list of a search may still have the recording selected.
    if (this.selectedId === id) await this.select(this.rows.find((row) => row.id !== id)?.id ?? null);
    return true;
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

  /** Asks for the rename of the speakers (focused on one speaker, when it is named): the speaker sheet answers it. */
  openRename(speaker?: string): void {
    this.renameFor = { speaker: speaker ?? null };
  }

  closeRename(): void {
    this.renameFor = null;
  }

  /**
   * Opens the picker of the recording, which is open already when it is asked again: only the error changes, and only when there is
   * a new one (the one that is shown is not answered by an ask for the picker).
   */
  openRecorder(error: ErrorView | null = null): void {
    this.recorder = { error: error ?? this.recorder?.error ?? null };
  }

  /** Sets the error that the open picker shows (what a start said), or clears it. A picker that is not open has none to show. */
  setRecorderError(error: ErrorView | null): void {
    if (this.recorder) this.recorder = { error };
  }

  closeRecorder(): void {
    this.recorder = null;
  }

  /** Ends the recording and starts its meeting: the "Stop" of the toolbar. A stop that fails says why in the notice. */
  async stopRecording(): Promise<void> {
    try {
      await api.recordingStop();
    } catch (e) {
      this.notify(errorMessage(asErrorView(e)));
    }
  }

  /** Opens the pane of System Settings where Screen Recording is turned on (macOS). */
  async openScreenPermission(): Promise<void> {
    try {
      await api.openPermissionSettings("screen");
    } catch (e) {
      this.notify(errorText(e));
    }
  }

  openLinkDialog(): void {
    this.linkDialogOpen = true;
  }

  closeLinkDialog(): void {
    this.linkDialogOpen = false;
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
    if (view.outcome !== null || before?.step !== view.step) void this.reloadDetail(true);
  }

  private async refreshAll(): Promise<void> {
    this.jobs = {};
    const before = this.selectedId;
    await this.load();
    // `load` has read the detail of the recording that took the place of one that is gone.
    if (before !== null && this.selectedId === before) await this.reloadDetail(true);
  }
}
