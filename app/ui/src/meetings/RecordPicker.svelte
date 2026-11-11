<script lang="ts">
  import { onMount, tick } from "svelte";
  import { api, asErrorView, errorText } from "../lib/api";
  import type { RecordMode, RecordOptions, RecordQuality, ScreenAvailability, SourcesView } from "../lib/api";
  import { Flash } from "../lib/flash.svelte";
  import { t } from "../lib/i18n";
  import { errorMessage } from "../lib/meeting-errors";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import { recordHotkey } from "../lib/recording";
  import Modal from "./Modal.svelte";

  // "Record & summarize": what to record and how, in a dialog over the window. It is there while the store has the picker open. What
  // the system offers is asked when it opens, once (a window that is opened later is not in the list); the choices of the last time
  // (`SourcesView.last`) are what it starts with.
  let { store }: { store: MeetingsStore } = $props();

  type Kind = RecordMode["kind"];

  const MODES: { kind: Kind; label: string }[] = [
    { kind: "screen", label: "recording.mode.screen" },
    { kind: "window", label: "recording.mode.window" },
    { kind: "audio", label: "recording.mode.audio" },
  ];
  const QUALITIES: { quality: RecordQuality; label: string; hint: string }[] = [
    { quality: "small", label: "recording.quality.small", hint: "recording.quality.small_hint" },
    { quality: "standard", label: "recording.quality.standard", hint: "recording.quality.standard_hint" },
    { quality: "sharp", label: "recording.quality.sharp", hint: "recording.quality.sharp_hint" },
  ];

  /** What stops the two modes with a picture: ffmpeg is not there or can't encode, or the desktop can't share the screen. */
  const BLOCKS_THE_SCREEN: ScreenAvailability[] = ["needs_ffmpeg", "no_video_encoder", "portal_unavailable"];

  /** What the system offers; null until it has said. */
  let sources = $state.raw<SourcesView | null>(null);
  /** Why it could not say, in a sentence. */
  let loadFailure = $state("");

  let kind = $state<Kind>("screen");
  /** The display that is chosen: empty until there is a list of more than one. */
  let displayId = $state("");
  /** The window that is chosen: empty is "Choose a window…". */
  let windowId = $state("");
  let quality = $state<RecordQuality>("standard");
  let systemAudio = $state(true);
  let mic = $state(true);
  /** The sources are being started: the desktop may be asking the person what to share. */
  let starting = $state(false);

  let modes: HTMLElement | undefined = $state();
  const copied = new Flash();
  $effect(() => () => copied.clear());

  const error = $derived(store.recorder?.error ?? null);
  const hotkey = recordHotkey();

  /** The two modes with a picture can't be chosen. */
  const screenBlocked = $derived(sources !== null && BLOCKS_THE_SCREEN.includes(sources.screen));

  const canStart = $derived.by(() => {
    if (sources === null || starting) return false;
    if (kind === "audio") return mic || systemAudio;
    if (screenBlocked) return false;
    // A desktop that asks the person what to share has no window to choose here.
    if (kind === "window") return sources.pickerIsTheSystem || windowId !== "";
    return true;
  });

  function apply(offered: SourcesView) {
    sources = offered;
    const last = offered.last;
    quality = last.quality;
    mic = last.mic;
    systemAudio = last.system_audio;
    // The mode of the last time may not be there any more (ffmpeg was removed): sound alone is what is left.
    kind = BLOCKS_THE_SCREEN.includes(offered.screen) && last.mode.kind !== "audio" ? "audio" : last.mode.kind;
    const wanted = last.mode.kind === "screen" ? last.mode.display : null;
    displayId = offered.displays.find((display) => display.id === wanted)?.id ?? offered.displays[0]?.id ?? "";
  }

  // A recording that begins somewhere else (the shortcut, the tray) is the bar's: this picker has nothing left to start, and a start
  // would only be refused. The recording that this one begins is not "somewhere else": the dialog waits for it.
  $effect(() => {
    if (store.isRecording && !starting) store.closeRecorder();
  });

  // The focus is in the dialog while it is open, on the mode that is chosen, and goes back to what had it.
  onMount(() => {
    const before = document.activeElement;
    let current = true;
    api.recordingSources().then(
      async (offered) => {
        if (!current) return;
        apply(offered);
        await tick();
        modes?.querySelector<HTMLInputElement>("input:checked")?.focus();
      },
      (e: unknown) => {
        if (current) loadFailure = errorText(e);
      },
    );
    return () => {
      current = false;
      if (before instanceof HTMLElement) before.focus();
    };
  });

  /** What the person chose, as the Rust side takes it. The main display is not named, so that it is the main one at the start. */
  function options(): RecordOptions {
    const first = sources?.displays[0]?.id ?? "";
    const mode: RecordMode =
      kind === "screen"
        ? { kind: "screen", display: displayId === "" || displayId === first ? null : displayId }
        : kind === "window"
          ? { kind: "window", id: windowId }
          : { kind: "audio" };
    return { mode, quality, mic, system_audio: systemAudio };
  }

  async function start() {
    if (!canStart) return;
    starting = true;
    store.setRecorderError(null);
    try {
      await api.recordingStart(options());
      // The bar is the recording's from here.
      store.closeRecorder();
    } catch (e) {
      store.setRecorderError(asErrorView(e));
      starting = false;
    }
  }

  /** What is being started is not called off from here: it ends in a recording, or in the error that this shows. */
  function cancel() {
    if (!starting) store.closeRecorder();
  }

  async function copy(text: string) {
    if (await store.copy(text)) copied.show();
  }

  /** Return is the default action wherever it isn't the action of the control that has the focus. */
  function onkeydown(event: KeyboardEvent) {
    if (event.key !== "Enter" || event.isComposing || event.defaultPrevented) return;
    if (event.target instanceof Element && event.target.closest("button, select, textarea, a")) return;
    if (!canStart) return;
    event.preventDefault();
    void start();
  }
</script>

<svelte:window {onkeydown} />

{#snippet icon(mode: Kind)}
  <svg class="icon" viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    {#if mode === "screen"}
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M8 20h8M12 16v4" />
    {:else if mode === "window"}
      <rect x="3" y="5" width="18" height="14" rx="2" />
      <path d="M3 9h18" />
    {:else}
      <path d="M4 10v4M8 7v10M12 4v16M16 8v8M20 11v2" />
    {/if}
  </svg>
{/snippet}

<Modal labelledby="record-title" onclose={cancel}>
  <div class="sheet">
    <h2 id="record-title">{t("recording.picker.title")}</h2>

    {#if loadFailure}
      <p class="error" role="alert">{loadFailure}</p>
      <div class="footer">
        <span class="spacer"></span>
        <button type="button" onclick={cancel}>{t("common.cancel")}</button>
      </div>
    {:else if sources}
      <div class="modes" role="radiogroup" aria-label={t("recording.picker.mode")} bind:this={modes}>
        {#each MODES as mode (mode.kind)}
          {@const blocked = mode.kind !== "audio" && screenBlocked}
          <label class="mode">
            <input type="radio" name="record-mode" value={mode.kind} checked={kind === mode.kind} disabled={blocked} onchange={() => (kind = mode.kind)} />
            <span class="card">
              {@render icon(mode.kind)}
              <span>{t(mode.label)}</span>
            </span>
          </label>
        {/each}
      </div>

      {#if sources.screen === "needs_permission"}
        <div class="note warn">
          <span>{t("recording.picker.needs_permission")}</span>
          <button type="button" onclick={() => store.openScreenPermission()}>{t("permissions.open_settings")}</button>
        </div>
      {:else if sources.screen === "needs_ffmpeg" || sources.screen === "no_video_encoder"}
        <div class="note warn">
          <span>{t(sources.screen === "needs_ffmpeg" ? "recording.error.needs_ffmpeg" : "recording.error.no_video_encoder", { command: sources.ffmpegCommand })}</span>
          <button type="button" onclick={() => copy(sources!.ffmpegCommand)}>{copied.on ? t("meetings.copied") : t("meetings.errorcard.copy_command")}</button>
        </div>
      {:else if sources.screen === "portal_unavailable"}
        <div class="note warn"><span>{t("recording.error.portal_unavailable")}</span></div>
      {/if}

      {#if kind !== "audio"}
        {#if sources.pickerIsTheSystem}
          <p class="hint">{t("recording.picker.system_asks")}</p>
          <p class="hint">{t("recording.picker.own_windows")}</p>
        {:else if kind === "screen" && sources.displays.length > 1}
          <label class="row">
            <span>{t("recording.picker.display")}</span>
            <select bind:value={displayId}>
              {#each sources.displays as display (display.id)}
                <option value={display.id}>{display.name}</option>
              {/each}
            </select>
          </label>
        {:else if kind === "window"}
          <label class="row">
            <span>{t("recording.picker.window")}</span>
            <select bind:value={windowId}>
              <option value="">{t("recording.picker.choose_window")}</option>
              {#each sources.windows as window (window.id)}
                <option value={window.id}>{window.name}</option>
              {/each}
            </select>
          </label>
        {/if}

        <div class="row quality">
          <span id="record-quality">{t("recording.picker.quality")}</span>
          <div class="segmented" role="radiogroup" aria-labelledby="record-quality">
            {#each QUALITIES as choice (choice.quality)}
              <label>
                <input type="radio" name="record-quality" value={choice.quality} checked={quality === choice.quality} onchange={() => (quality = choice.quality)} />
                <span>{t(choice.label)}</span>
              </label>
            {/each}
          </div>
        </div>
        <p class="hint">{t(QUALITIES.find((choice) => choice.quality === quality)?.hint ?? "recording.quality.standard_hint")}</p>
      {/if}

      <div class="sounds">
        <label class="check"><input type="checkbox" bind:checked={systemAudio} /> <span>{t("recording.picker.system_audio")}</span></label>
        <label class="check"><input type="checkbox" bind:checked={mic} /> <span>{t("recording.picker.mic")}</span></label>
      </div>

      <div class="footer">
        <span class="hint caption">{t("recording.picker.hotkey_hint", { hotkey })}</span>
        <button type="button" disabled={starting} aria-keyshortcuts="Escape" onclick={cancel}>{t("common.cancel")}</button>
        <button type="button" class="record" disabled={!canStart} aria-keyshortcuts="Enter" onclick={start}>
          {starting ? t("recording.picker.starting") : t("recording.picker.start")}
        </button>
      </div>

      {#if error}
        <div class="problem">
          <p class="error" role="alert">{errorMessage(error)}</p>
          {#if error.command === "open_settings"}
            <button type="button" onclick={() => store.openScreenPermission()}>{t("permissions.open_settings")}</button>
          {:else if error.command}
            <button type="button" onclick={() => copy(error.command ?? "")}>{copied.on ? t("meetings.copied") : t("meetings.errorcard.copy_command")}</button>
          {/if}
        </div>
      {/if}
    {/if}
  </div>
</Modal>

<style>
  .sheet {
    display: flex;
    flex-direction: column;
    gap: 12px;
    width: min(520px, calc(100vw - 64px));
    padding: 6px 4px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
    letter-spacing: normal;
    text-transform: none;
    color: var(--fg);
  }
  .modes {
    display: flex;
    gap: 8px;
  }
  .mode {
    position: relative;
    flex: 1;
    min-width: 0;
  }
  .mode input {
    position: absolute;
    inset: 0;
    margin: 0;
    opacity: 0;
    cursor: pointer;
  }
  .card {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    min-height: 64px;
    padding: 8px;
    text-align: center;
    border: 1px solid var(--border);
    border-radius: 10px;
  }
  .mode input:checked + .card {
    color: var(--accent);
    background: var(--accent-soft);
    border: 2px solid var(--accent);
    padding: 7px;
  }
  .mode input:focus-visible + .card {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .mode input:disabled {
    cursor: default;
  }
  .mode input:disabled + .card {
    opacity: 0.45;
  }
  .note {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 8px;
    overflow-wrap: anywhere;
  }
  .note.warn {
    color: var(--warn-fg);
    background: var(--warn-bg);
  }
  .note button {
    flex: none;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .row select {
    flex: 0 1 300px;
    min-width: 0;
  }
  .hint {
    margin: 0;
  }
  .segmented {
    display: flex;
  }
  .segmented label {
    position: relative;
  }
  .segmented input {
    position: absolute;
    inset: 0;
    margin: 0;
    opacity: 0;
    cursor: pointer;
  }
  .segmented span {
    display: block;
    padding: 4px 14px;
    border: 1px solid var(--border);
    margin-left: -1px;
  }
  .segmented label:first-child span {
    margin-left: 0;
    border-radius: 6px 0 0 6px;
  }
  .segmented label:last-child span {
    border-radius: 0 6px 6px 0;
  }
  .segmented input:checked + span {
    color: var(--accent-fg);
    background: var(--accent);
    border-color: var(--accent);
  }
  .segmented input:focus-visible + span {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .sounds {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .footer {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .caption,
  .spacer {
    flex: 1;
    min-width: 0;
  }
  .record {
    color: #fff;
    background: #d92d20;
    border-color: #d92d20;
  }
  .problem {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  .problem .error {
    margin: 0;
  }
  .problem button {
    flex: none;
  }
</style>
