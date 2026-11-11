<script lang="ts">
  import { api } from "../lib/api";
  import type { RecordingNote, RecordingTrack, RecordingView } from "../lib/api";
  import { formatClock } from "../lib/days";
  import { setLanguage, t } from "../lib/i18n";
  import { barHeight, barsLit, hearsSound } from "../lib/levels";
  import { IDLE_RECORDING, recordHotkey } from "../lib/recording";

  // The recording bar: a small capsule over the other windows that shows how the recording goes (the time, the meters) and has its
  // buttons. Its window is made by the Rust side in the middle of the first recording, after the first states were sent: the page
  // asks for the state when it opens, and hears the ones that follow. The window is hidden between recordings and this page stays,
  // so what it shows is whatever the recorder last said.

  const NOTES: Record<RecordingNote["kind"], { text: string; tip: string }> = {
    silent: { text: "recording.hud.note.silent", tip: "recording.hud.note.silent_tip" },
    restarted: { text: "recording.hud.note.restarted", tip: "recording.hud.note.restarted_tip" },
    off: { text: "recording.hud.note.off", tip: "recording.hud.note.off_tip" },
  };
  /** The tracks that have a meter, in the order the capsule has them, and the words of their names. */
  const METERED: { track: "system" | "mic"; name: string }[] = [
    { track: "system", name: "recording.hud.system" },
    { track: "mic", name: "recording.hud.mic" },
  ];
  const NAMES: Record<RecordingTrack, string> = {
    system: "recording.hud.system",
    mic: "recording.hud.mic",
    screen: "recording.hud.screen",
  };

  let view = $state.raw<RecordingView>(IDLE_RECORDING);
  /** The question "Discard this recording?" is asked. The recording goes on while it is. */
  let confirming = $state(false);
  /** A button that asked the recorder for something is waiting for it. */
  let busy = $state(false);

  const hotkey = recordHotkey();
  const paused = $derived(view.phase === "paused");
  const finishing = $derived(view.phase === "finishing");
  const asking = $derived(confirming && !finishing);
  /** Both meters are on: there is room for their bars, and not for their names (the tooltips and the labels have them). */
  const compact = $derived(view.levels.system !== null && view.levels.mic !== null);
  const notes = $derived(view.notes);

  // The words follow the language of the settings, which the bar can't read. It is asked when the recording begins, and the state
  // that shows the bar waits for the answer: the bar is not drawn in the language of the time before. States that come meanwhile wait
  // behind it, so that they are applied in the order they were sent.
  async function refreshLanguage() {
    try {
      setLanguage(await api.panelLanguage());
    } catch (e) {
      console.warn("Could not ask for the language:", e);
    }
  }

  let applied: Promise<void> = Promise.resolve();

  function apply(next: RecordingView) {
    applied = applied.then(async () => {
      const begins = view.phase === "idle" && next.phase !== "idle";
      if (begins) await refreshLanguage();
      // A question left open belongs to the recording before.
      if (begins || next.phase === "idle") confirming = false;
      view = next;
    });
  }

  // A page that loads after the recording began never heard its states: it asks for the state once it listens. A state that comes by
  // the event meanwhile is newer than the answer, and the answer is dropped.
  $effect(() => {
    let stopped = false;
    let stop: (() => void) | undefined;
    let heard = false;
    api
      .onRecordingState((state) => {
        heard = true;
        apply(state);
      })
      .then((unlisten) => {
        if (stopped) {
          unlisten();
          return;
        }
        stop = unlisten;
        return api.recordingState().then((state) => {
          if (!stopped && !heard) apply(state);
        });
      })
      .catch((e: unknown) => console.warn("Could not hear the recording:", e));
    return () => {
      stopped = true;
      stop?.();
    };
  });

  /**
   * What a button asks of the recorder. One at a time, and a failure is not this page's to say: the app tells it in a notification
   * (a recording that could not be put together), and the state that follows is the news.
   */
  async function ask(call: () => Promise<void>) {
    if (busy) return;
    busy = true;
    try {
      await call();
    } catch (e) {
      console.warn("The recorder did not do what the bar asked:", e);
    } finally {
      busy = false;
    }
  }

  function discard() {
    confirming = false;
    void ask(() => api.recordingDiscard());
  }

  /** Esc is "Keep recording". (The bar never takes the focus, so this is for a system that gives it the keys all the same.) */
  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && confirming) {
      event.preventDefault();
      confirming = false;
    }
  }

  const noteOf = (track: RecordingTrack) => notes.find((entry) => entry.track === track)?.note;
  const tooltip = (name: string, level: number | null) =>
    t(hearsSound(level) ? "recording.hud.hearing" : "recording.hud.quiet", { name });
</script>

<svelte:window {onkeydown} />

{#snippet chip(track: RecordingTrack, note: RecordingNote)}
  <span class="note {note.kind}" title={t(NOTES[note.kind].tip, { name: t(NAMES[track]) })} data-tauri-drag-region>
    {t(NOTES[note.kind].text, { name: t(NAMES[track]) })}
  </span>
{/snippet}

<main>
  {#if view.phase !== "idle"}
    <div class="capsule" data-tauri-drag-region>
      <span
        class="dot"
        class:paused
        role="img"
        aria-label={paused ? t("recording.hud.paused") : t("recording.hud.recording")}
        data-tauri-drag-region
      ></span>
      <span class="time" data-tauri-drag-region>{formatClock(view.elapsedMs / 1000)}</span>

      {#if asking}
        <span class="question" data-tauri-drag-region>{t("recording.hud.discard_ask")}</span>
        <button type="button" class="pill danger" onclick={discard}>{t("recording.hud.discard_yes")}</button>
        <button type="button" class="pill keep" onclick={() => (confirming = false)}>{t("recording.hud.keep")}</button>
      {:else}
        <span class="divider" aria-hidden="true" data-tauri-drag-region></span>
        <div class="meters" class:compact data-tauri-drag-region>
          {#each METERED as { track, name } (track)}
            {@const level = view.levels[track]}
            {@const note = noteOf(track)}
            {#if level !== null}
              {@const label = tooltip(t(name), level)}
              <div class="meter" role="img" aria-label={label} title={label} data-tauri-drag-region>
                <svg class="icon" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  {#if track === "system"}
                    <path d="M4 10v4M8 7v10M12 4v16M16 8v8M20 11v2" />
                  {:else}
                    <rect x="9" y="3" width="6" height="11" rx="3" fill="currentColor" />
                    <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
                  {/if}
                </svg>
                <span class="label">{t(name)}</span>
                <span class="bars" aria-hidden="true">
                  {#each barsLit(level) as lit, i (i)}
                    <i class="bar" class:lit class:last={i === 5} style:height="{barHeight(i)}px"></i>
                  {/each}
                </span>
              </div>
            {/if}
            {#if note}{@render chip(track, note)}{/if}
          {/each}
          {#if noteOf("screen")}{@render chip("screen", noteOf("screen")!)}{/if}
        </div>

        <button
          type="button"
          class="round"
          disabled={busy || finishing}
          aria-label={paused ? t("recording.hud.resume") : t("recording.hud.pause")}
          title={paused ? t("recording.hud.resume") : t("recording.hud.pause")}
          onclick={() => ask(() => (paused ? api.recordingResume() : api.recordingPause()))}
        >
          <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" aria-hidden="true">
            {#if paused}
              <path d="M7 4.5v15l12-7.5z" />
            {:else}
              <path d="M6 4h4v16H6zM14 4h4v16h-4z" />
            {/if}
          </svg>
        </button>
        <button
          type="button"
          class="stop"
          disabled={busy || finishing}
          title={t("recording.hud.stop_tip", { hotkey })}
          onclick={() => ask(() => api.recordingStop())}
        >
          <span class="square" aria-hidden="true"></span>
          {finishing ? t("recording.hud.saving") : t("recording.hud.stop")}
        </button>
        <button
          type="button"
          class="trash"
          disabled={busy || finishing}
          aria-label={t("recording.hud.discard")}
          title={t("recording.hud.discard")}
          onclick={() => (confirming = true)}
        >
          <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" />
          </svg>
        </button>
      {/if}
    </div>
  {/if}
</main>

<style>
  main {
    display: flex;
    justify-content: center;
    padding: 6px 12px;
  }
  .capsule {
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: 100%;
    height: 52px;
    padding: 0 10px 0 16px;
    white-space: nowrap;
    background: #1c1c1c;
    border-radius: 26px;
    box-shadow: 0 4px 10px rgb(0 0 0 / 0.3);
  }
  .dot {
    flex: none;
    width: 10px;
    height: 10px;
    background: #ff453a;
    border-radius: 50%;
  }
  .dot.paused {
    background: #ff9f0a;
  }
  .time {
    flex: none;
    font-size: 15px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .divider {
    flex: none;
    width: 1px;
    height: 24px;
    background: rgb(255 255 255 / 0.2);
  }
  .question {
    font-weight: 600;
  }
  /* The meters give way before the buttons do: a bar that is too narrow for them cuts them, not the buttons. */
  .meters {
    display: flex;
    flex: 0 1 auto;
    align-items: center;
    gap: 12px;
    min-width: 0;
    overflow: hidden;
  }
  .meter {
    display: flex;
    flex: none;
    align-items: center;
    gap: 4px;
  }
  .meter > * {
    pointer-events: none;
  }
  .meters.compact .label {
    display: none;
  }
  .bars {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 16px;
  }
  .bar {
    width: 3px;
    background: rgb(255 255 255 / 0.2);
    border-radius: 1px;
    transition: background-color 0.08s linear;
  }
  .bar.lit {
    background: #30d158;
  }
  .bar.last.lit {
    background: #ff9f0a;
  }
  .note {
    flex: none;
    font-size: 11px;
    font-weight: 600;
    color: #ffd60a;
  }
  .note::before {
    content: "⚠ ";
  }
  /* A track that is off records nothing for the rest of the recording, and a recording of sound alone may have no other: it is the
     one thing on the bar that must not be missed. */
  .note.off {
    padding: 3px 9px;
    font-weight: 700;
    color: #fff;
    background: #d92d20;
    border-radius: 999px;
  }
  button {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 0;
    border: 0;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .round {
    width: 32px;
    height: 32px;
    background: rgb(255 255 255 / 0.14);
    border-radius: 50%;
  }
  .stop {
    height: 32px;
    padding: 0 14px;
    font-weight: 600;
    color: #000;
    background: #fff;
    border-radius: 16px;
  }
  .square {
    width: 10px;
    height: 10px;
    background: #000;
    border-radius: 2px;
  }
  .trash {
    width: 28px;
    height: 28px;
    color: rgb(255 255 255 / 0.7);
    background: transparent;
    border-radius: 50%;
  }
  .trash:hover:not(:disabled) {
    color: #ff453a;
    background: rgb(255 255 255 / 0.1);
  }
  .pill {
    height: 32px;
    padding: 0 14px;
    font-weight: 600;
    border-radius: 16px;
  }
  .pill.danger {
    color: #fff;
    background: #d92d20;
  }
  .pill.keep {
    background: rgb(255 255 255 / 0.14);
  }
</style>
