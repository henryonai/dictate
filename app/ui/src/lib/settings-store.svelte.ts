// What the settings window knows about the settings, and the one way it changes them.
//
// A change shows on screen at once and goes to the Rust side behind the scenes; if the Rust side refuses it,
// that change goes back and the reason is kept in `error`. Calls go one after the other, so a later change never
// overtakes an earlier one.
//
// What is on screen is always what the Rust side last confirmed with the changes that it hasn't answered yet laid
// on top, oldest first. So an answer never wipes out a change made while the call was on its way, and the last
// change to a setting wins. A call sends the same, but only up to its own change: the ones made after it come with
// their own calls.
import { api, errorText } from "./api";
import type { Account, AiProvider, HotkeyStatus, Settings, SettingsView, TranscribePreset } from "./api";
import { setLanguage } from "./i18n";

/** The settings without what the view adds to them. */
function settingsOf(view: SettingsView): Settings {
  const { has_ai_key, has_transcribe_key, shares_openrouter_key, load_warning, system_language, ...settings } = view;
  return settings;
}

/** A call to the Rust side, and the changes it was started for, which show on screen until it is answered. */
interface Pending {
  patch: Partial<Settings>;
  /** How many calls had failed when this one was queued; see `send`. */
  failuresBefore: number;
}

export class SettingsStore {
  /** Null until the first load. */
  view = $state.raw<SettingsView | null>(null);
  /**
   * Why the settings file wasn't used as it was. The Rust side tells only once, with the first view; this keeps
   * it for as long as it is shown and never lets a later view without it wipe it out.
   */
  loadWarning = $state<string | null>(null);
  /** What went wrong last, as the Rust side said it; empty when all is well. */
  error = $state("");
  /** True while the settings are being read; the window has nothing to show before that has worked once. */
  loading = $state(true);
  /** What the Rust side says about the shortcut; null until it has said, and for good if it can't. */
  hotkey = $state.raw<HotkeyStatus | null>(null);

  /** What the Rust side last confirmed. */
  private saved: SettingsView | null = null;
  private pending: Pending[] = [];
  private failures = 0;
  private queue: Promise<unknown> = Promise.resolve();

  async load(): Promise<void> {
    this.loading = true;
    this.error = "";
    try {
      const view = await api.getSettings();
      this.saved = view;
      this.loadWarning = view.load_warning ?? this.loadWarning;
      this.show();
      void this.refreshHotkey();
    } catch (e) {
      this.error = errorText(e);
    } finally {
      this.loading = false;
    }
  }

  /** Asks how the shortcut stands. Without an answer the window shows the shortcut as the settings have it. */
  private async refreshHotkey(): Promise<void> {
    try {
      this.hotkey = await api.hotkeyStatus();
    } catch {
      // Nothing to tell: the settings' own text of the shortcut stands in.
    }
  }

  /** Follows what the Rust side says about the shortcut by itself (the desktop answers the portal late). Resolves with the function that stops it. */
  watchHotkey(): Promise<() => void> {
    return api.onHotkeyStatus((status) => {
      this.hotkey = status;
    });
  }

  dismissWarning(): void {
    this.loadWarning = null;
  }

  dismissError(): void {
    this.error = "";
  }

  /** Saves a change to the settings. Whether it was saved. */
  update(patch: Partial<Settings>): Promise<boolean> {
    if (!this.saved) return Promise.resolve(false);
    // The call sends the settings as they are when it starts, with its own change on them: never the changes made
    // after it, which would go to the Rust side before their turn.
    // `saved` is set: that was checked above and it never goes back to null.
    return this.send(patch, (settings) => api.saveSettings(settings as Settings));
  }

  /** The Rust side moves the base URL along with the provider, so its answer decides what shows. */
  setAiProvider(provider: AiProvider): Promise<boolean> {
    return this.send({ ai_provider: provider }, () => api.setAiProvider(provider));
  }

  /** Same for the transcription service: its base URL and models come with the answer. */
  setTranscribePreset(preset: TranscribePreset): Promise<boolean> {
    return this.send({ transcribe_preset: preset }, () => api.setTranscribePreset(preset));
  }

  /**
   * The Rust side registers the shortcut, and saves it only when that worked. Resolves with "" when it did, else
   * with why it didn't: a key of the texts (`hotkey.taken`, `hotkey.invalid`) or a sentence. The recorder shows
   * that next to the shortcut, so it is not also a banner of the window.
   */
  async setHotkey(accel: string): Promise<string> {
    let failure = "";
    const saved = await this.send(
      { hotkey: accel },
      () => api.setHotkey(accel),
      (e) => {
        failure = errorText(e);
      },
    );
    if (saved) await this.refreshHotkey();
    return saved ? "" : failure;
  }

  /** An empty `value` deletes the key. The answer says whether a key is saved, never what it is. */
  setSecret(account: Account, value: string): Promise<boolean> {
    return this.send({}, () => api.setSecret(account, value));
  }

  /** Resolves when every call made so far has ended. */
  async settled(): Promise<void> {
    await this.queue;
  }

  /**
   * What the Rust side confirmed with the changes still waiting for it laid on top, oldest first. With `last`, only
   * up to and including that change.
   */
  private laidOn(saved: SettingsView, last?: Pending): SettingsView {
    let view = saved;
    for (const entry of this.pending) {
      view = { ...view, ...entry.patch };
      if (entry === last) break;
    }
    return view;
  }

  /** Puts what the Rust side confirmed, and the changes still waiting for it, on screen. */
  private show(): void {
    if (!this.saved) return;
    const view = this.laidOn(this.saved);
    this.view = view;
    setLanguage(view.ui_language, view.system_language);
  }

  /**
   * `call` gets the settings to send as the call starts; the calls that don't send settings ignore them. A failure
   * goes to `onFailure` when there is one, and is the window's `error` otherwise.
   */
  private send(
    patch: Partial<Settings>,
    call: (settings: Settings | null) => Promise<SettingsView>,
    onFailure?: (error: unknown) => void,
  ): Promise<boolean> {
    const entry: Pending = { patch, failuresBefore: this.failures };
    this.pending.push(entry);
    this.show();
    const sent = this.queue.then(async () => {
      try {
        this.saved = await call(this.saved && settingsOf(this.laidOn(this.saved, entry)));
        this.loadWarning = this.saved.load_warning ?? this.loadWarning;
        // A call that was queued before a failure doesn't make that failure go away.
        if (this.failures === entry.failuresBefore) this.error = "";
        return true;
      } catch (e) {
        if (onFailure) {
          onFailure(e);
        } else {
          this.failures += 1;
          this.error = errorText(e);
        }
        return false;
      } finally {
        this.pending = this.pending.filter((other) => other !== entry);
        this.show();
      }
    });
    this.queue = sent;
    return sent;
  }
}
