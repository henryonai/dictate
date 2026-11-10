// The player of a recording: the media element, and what keeps it playing.
//
// The recording is played from the media server of the app, at `http://127.0.0.1:<port>/<token>/<n>`. The port and the token change
// with every launch of the app and when the server is built again, so an address is never kept: it is the one the last detail
// had, and the player reads it again when something is wrong. Three things tell that something may be:
//   1. the element's `error` event: the address is read again, the new one is played and the position goes back to where it was;
//   2. a play or a seek after a pause (the server may have been rebuilt while nobody listened): if the address is another one it is
//      swapped in, at the same position;
//   3. `waiting` or `stalled` for about 8 s: the same as for the error.
// An error with the address that is still the right one is a file that the element can't play (a `.mkv`): the player says so.
import { api } from "./api";
import type { MediaView } from "./api";
import { onLinux } from "../panel/state";

/** How often the position is read. */
export const TICK_MS = 500;
/** How long an element may wait for data (`waiting`, `stalled`) before the address is read again. */
export const STALL_MS = 8000;
/** How many new addresses in a row may fail before the file is given up. */
const MAX_RECOVERIES = 3;

/** The media of a recording as the Rust side has it now: another address than before when the server was built again. */
export type FetchMedia = (id: string) => Promise<MediaView | null>;

const fromServer: FetchMedia = async (id) => (await api.meetingDetail(id)).media;

/** What the player does with each event of the element. */
type Handler = () => void;

export class PlayerController {
  /** Seconds, as of the last reading (every half second, and at once after a seek). */
  currentTime = $state(0);
  /** The element is playing: not paused, not at the end. */
  playing = $state(false);
  /** The element can't play the file, and no other address helped. */
  failed = $state(false);
  /** The media has a picture. */
  video = $state(false);
  /** Where the recording plays from: empty when it has no media, or when the player let go of it. */
  url = $state("");

  /** There is something to play and nothing has failed: the element is shown, and a seek is heard. */
  get available(): boolean {
    return this.url !== "" && !this.failed;
  }

  private readonly fetchMedia: FetchMedia;
  private id: string | null = null;
  private element: HTMLMediaElement | null = null;
  private handlers: Record<string, Handler> = {};
  private ticker: ReturnType<typeof setInterval> | undefined;
  private backstop: ReturnType<typeof setTimeout> | undefined;
  /** Where to go back to, and whether to play on, when the address that was just set has loaded: setting one starts at 0. */
  private pending: { at: number; resume: boolean } | null = null;
  /** The element has been paused since the last time the address was looked at (it starts so: the first play counts). */
  private wasPaused = true;
  private recovering = false;
  private recoveries = 0;
  private released = false;

  constructor(fetchMedia: FetchMedia = fromServer) {
    this.fetchMedia = fetchMedia;
  }

  /**
   * The recording the player is on and its media (from the detail). Another recording starts from its beginning. The same one with
   * another address (the server was built again) goes on from where it was.
   */
  bind(id: string | null, media: MediaView | null): void {
    if (id !== this.id) {
      this.id = id;
      this.reset();
      this.url = media?.url ?? "";
      this.video = media?.video ?? false;
      if (this.element && this.url) this.element.src = this.url;
      return;
    }
    if (media) this.video = media.video;
    const url = media?.url ?? "";
    if (url === this.url) return;
    if (url === "") {
      this.url = "";
      return;
    }
    // The same recording at another address: from where it was, and playing if it was.
    const element = this.element;
    const live = element !== null && !this.pending;
    this.swap(url, live ? element.currentTime : this.currentTime, live && !element.paused);
  }

  /** The element to drive. Gives back the function that lets it go. */
  attach(element: HTMLMediaElement): () => void {
    this.detach();
    this.element = element;
    this.released = false;
    this.handlers = {
      play: () => this.onPlay(),
      pause: () => this.onPause(),
      ended: () => this.onPause(),
      playing: () => this.clearBackstop(),
      canplay: () => this.clearBackstop(),
      emptied: () => this.clearBackstop(),
      loadedmetadata: () => this.onLoaded(),
      waiting: () => this.startBackstop(),
      stalled: () => this.startBackstop(),
      error: () => {
        if (!this.released) void this.recover(true);
      },
    };
    for (const [type, handler] of Object.entries(this.handlers)) element.addEventListener(type, handler);
    this.ticker = setInterval(() => this.tick(), TICK_MS);
    if (this.url) element.src = this.url;
    return () => this.detach(element);
  }

  /** Goes to the second, exactly, and plays. Heard only when there is a player. */
  async seek(seconds: number): Promise<void> {
    const element = this.element;
    if (!element || this.released || !this.available) return;
    this.currentTime = seconds;
    if (this.redirect(seconds)) return;
    // After a pause the server may be another one: its address is read first.
    const fresh = element.paused ? await this.fresher() : null;
    if (this.element !== element || this.released || this.redirect(seconds)) return;
    // This is the look at the address for this play: the `play` event that follows has no need of another.
    this.wasPaused = false;
    if (fresh) {
      this.swap(fresh.url, seconds, true);
      return;
    }
    element.currentTime = seconds;
    this.play(element);
  }

  /**
   * A new address that has not loaded is at 0, and would forget a position that it is given now: it is told to go there (and play)
   * when it has loaded, in place of where it was to go. True when it was so.
   */
  private redirect(seconds: number): boolean {
    if (!this.pending) return false;
    this.pending = { at: seconds, resume: true };
    return true;
  }

  /**
   * Lets go of the file: stops, takes the address away and loads nothing. Windows may refuse to move a folder to the Trash while the
   * player has a file of it open. Gives back what puts the player as it was (at the same position, paused) for when the recording
   * stays; it does nothing once the player is on another recording.
   */
  release(): () => void {
    const element = this.element;
    if (element && !this.pending) this.currentTime = element.currentTime;
    const id = this.id;
    const url = this.url;
    const at = this.currentTime;
    this.released = true;
    this.clearBackstop();
    this.pending = null;
    this.url = "";
    this.playing = false;
    if (element) {
      element.pause();
      element.removeAttribute("src");
      element.load();
    }
    return () => {
      if (this.id === id && this.url === "" && url !== "") this.swap(url, at, false);
    };
  }

  private reset(): void {
    this.clearBackstop();
    this.pending = null;
    this.currentTime = 0;
    this.playing = false;
    this.failed = false;
    this.video = false;
    this.url = "";
    this.wasPaused = true;
    this.recoveries = 0;
    this.released = false;
  }

  private detach(only?: HTMLMediaElement): void {
    const element = this.element;
    if (!element || (only && only !== element)) return;
    for (const [type, handler] of Object.entries(this.handlers)) element.removeEventListener(type, handler);
    this.handlers = {};
    clearInterval(this.ticker);
    this.ticker = undefined;
    this.clearBackstop();
    this.element = null;
  }

  private tick(): void {
    const element = this.element;
    if (!element || this.released) return;
    // While a new address loads the element is at 0: that is not where the recording is.
    if (!this.pending) this.currentTime = element.currentTime;
    this.playing = !element.paused && !element.ended;
  }

  private onPlay(): void {
    if (this.released) return;
    this.playing = true;
    if (!this.wasPaused) return;
    this.wasPaused = false;
    void this.verify();
  }

  private onPause(): void {
    this.wasPaused = true;
    this.playing = false;
    this.clearBackstop();
    if (this.element && !this.pending && !this.released) this.currentTime = this.element.currentTime;
  }

  private onLoaded(): void {
    this.clearBackstop();
    this.recoveries = 0;
    const element = this.element;
    const pending = this.pending;
    this.pending = null;
    if (!element || !pending) return;
    element.currentTime = pending.at;
    this.currentTime = pending.at;
    if (pending.resume) this.play(element);
  }

  private startBackstop(): void {
    if (this.backstop !== undefined || this.released) return;
    this.backstop = setTimeout(() => {
      this.backstop = undefined;
      void this.recover(false);
    }, STALL_MS);
  }

  private clearBackstop(): void {
    clearTimeout(this.backstop);
    this.backstop = undefined;
  }

  /** Plays; a play that the system refuses (or that a new address cuts off) is not an error of the window. */
  private play(element: HTMLMediaElement): void {
    void Promise.resolve(element.play()).catch(() => {});
  }

  /** Plays the address from the start, and goes to `at` once it has loaded. */
  private swap(url: string, at: number, resume: boolean): void {
    this.url = url;
    this.released = false;
    this.pending = { at, resume };
    this.clearBackstop();
    if (this.element) this.element.src = url;
  }

  /** The address the server gives now, when it is not the one that is played; null when it is the same, or cannot be known. */
  private async fresher(): Promise<MediaView | null> {
    const id = this.id;
    if (id === null) return null;
    try {
      const media = await this.fetchMedia(id);
      return this.id === id && media && media.url !== this.url ? media : null;
    } catch {
      return null;
    }
  }

  /** A play after a pause: the address is looked at, and a new one is swapped in at the same position. */
  private async verify(): Promise<void> {
    const element = this.element;
    const fresh = await this.fresher();
    if (!fresh || this.element !== element || !element || this.released) return;
    this.swap(fresh.url, element.currentTime, !element.paused);
  }

  /**
   * The element failed (`fatal`: it said so) or waited too long. A new address is played from where it was. With the address that is
   * still the right one an error is a file that can't be played; a wait is nothing to do anything about.
   */
  private async recover(fatal: boolean): Promise<void> {
    const element = this.element;
    const id = this.id;
    if (!element || id === null || this.recovering || this.failed || this.released) return;
    this.recovering = true;
    let media: MediaView | null = null;
    try {
      media = await this.fetchMedia(id);
    } catch {
      media = null;
    } finally {
      this.recovering = false;
    }
    if (this.element !== element || this.id !== id || this.released) return;
    if (media && media.url !== this.url) {
      if (this.recoveries >= MAX_RECOVERIES) {
        this.failed = true;
        return;
      }
      this.recoveries += 1;
      this.swap(media.url, element.currentTime, !element.paused);
    } else if (fatal) {
      this.failed = true;
    }
  }
}

/** What the system needs to play the audio of a recording, as `audio.m4a` has it (AAC in MP4). */
const AAC = 'audio/mp4; codecs="mp4a.40.2"';

/**
 * Linux has no AAC decoder in a WebKitGTK that has no GStreamer libav: the recordings don't play, and a package fixes it. Only Linux
 * is asked; the other systems have a decoder of their own.
 */
export function needsLibav(): boolean {
  return onLinux() && document.createElement("audio").canPlayType(AAC) === "";
}
