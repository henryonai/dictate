// What the settings window knows about the settings, and the one way it changes them.
//
// A change shows on screen at once and goes to the Rust side behind the scenes; if the Rust side refuses it,
// the screen goes back to what is really saved and the reason is kept in `error`. Calls go one after the other,
// so a later change never overtakes an earlier one.
import { api, errorText } from "./api";
import type { Account, AiProvider, Settings, SettingsView, TranscribePreset } from "./api";
import { setLanguage } from "./i18n";

/** The settings without what the view adds to them. */
function settingsOf(view: SettingsView): Settings {
  const { has_ai_key, has_transcribe_key, shares_openrouter_key, load_warning, ...settings } = view;
  return settings;
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

  /** What the Rust side last confirmed. */
  private saved: SettingsView | null = null;
  private queue: Promise<unknown> = Promise.resolve();
  private waiting = 0;

  async load(): Promise<void> {
    try {
      const view = await api.getSettings();
      this.saved = view;
      this.loadWarning = view.load_warning ?? this.loadWarning;
      this.error = "";
      this.show(view);
    } catch (e) {
      this.error = errorText(e);
    }
  }

  dismissWarning(): void {
    this.loadWarning = null;
  }

  dismissError(): void {
    this.error = "";
  }

  /** Saves a change to the settings. Whether it was saved. */
  update(patch: Partial<Settings>): Promise<boolean> {
    if (this.view) this.show({ ...this.view, ...patch });
    return this.send(() => {
      // The newest wish, which may include changes made while an earlier save was on its way.
      return api.saveSettings(settingsOf(this.view as SettingsView));
    });
  }

  /** The Rust side moves the base URL along with the provider, so its answer decides what shows. */
  setAiProvider(provider: AiProvider): Promise<boolean> {
    if (this.view) this.show({ ...this.view, ai_provider: provider });
    return this.send(() => api.setAiProvider(provider));
  }

  /** Same for the transcription service: its base URL and models come with the answer. */
  setTranscribePreset(preset: TranscribePreset): Promise<boolean> {
    if (this.view) this.show({ ...this.view, transcribe_preset: preset });
    return this.send(() => api.setTranscribePreset(preset));
  }

  /** An empty `value` deletes the key. The answer says whether a key is saved, never what it is. */
  setSecret(account: Account, value: string): Promise<boolean> {
    return this.send(() => api.setSecret(account, value));
  }

  /** Resolves when every call made so far has ended. */
  async settled(): Promise<void> {
    await this.queue;
  }

  private show(view: SettingsView): void {
    this.view = view;
    setLanguage(view.ui_language);
  }

  private send(call: () => Promise<SettingsView>): Promise<boolean> {
    this.waiting += 1;
    const sent = this.queue.then(async () => {
      // With another call behind this one, its answer will be the one to show.
      const last = this.waiting === 1;
      try {
        const view = await call();
        this.saved = view;
        this.loadWarning = view.load_warning ?? this.loadWarning;
        this.error = "";
        if (last) this.show(view);
        return true;
      } catch (e) {
        this.error = errorText(e);
        if (last && this.saved) this.show(this.saved);
        return false;
      } finally {
        this.waiting -= 1;
      }
    });
    this.queue = sent;
    return sent;
  }
}
