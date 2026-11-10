<script lang="ts">
  import { untrack } from "svelte";
  import { formatDuration } from "../lib/days";
  import { Flash } from "../lib/flash.svelte";
  import { currentLanguage, t } from "../lib/i18n";
  import { transcriptText } from "../lib/meeting-text";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import { PlayerController } from "../lib/player.svelte";
  import DetailMenu from "./DetailMenu.svelte";
  import ErrorCard from "./ErrorCard.svelte";
  import ExportMenu from "./ExportMenu.svelte";
  import Player from "./Player.svelte";
  import Progress from "./Progress.svelte";
  import SummaryPane from "./SummaryPane.svelte";
  import Transcript from "./Transcript.svelte";

  // The selected recording: its header (the title, where it came from, and the buttons that act on it), then what it is doing or why
  // it stopped, and, when it is not at work, the player and the transcript on the left with the summary on the right.
  let { store }: { store: MeetingsStore } = $props();

  const row = $derived(store.selectedRow);
  /** The detail, when it is the selected recording's: the store drops it while another one is being read. */
  const detail = $derived(store.detail && store.detail.row.id === store.selectedId ? store.detail : null);

  // One player for the window: it is on the recording whose detail is shown, and goes on from where it was when only the address of
  // its media changes. `bind` reads the player's own state: it is not a dependency of this effect.
  const player = new PlayerController();
  $effect(() => {
    const shown = detail;
    untrack(() => player.bind(shown?.row.id ?? null, shown?.media ?? null));
  });

  const seek = (seconds: number) => void player.seek(seconds);

  /** "planning.mp4 · 3/2/26, 10:00 AM · 12 min": where it came from, when it was made, how long it is. */
  const meta = $derived.by(() => {
    if (!row) return "";
    const source = row.kind === "screen" ? t("meetings.screen_recording") : (detail?.sourceLabel ?? "");
    const made = row.created
      ? new Date(row.created * 1000).toLocaleString(currentLanguage(), { dateStyle: "short", timeStyle: "short" })
      : "";
    const length = row.duration === null ? "" : formatDuration(row.duration);
    return [source, made, length].filter(Boolean).join(" · ");
  });

  /** Something to show beside each other: a transcript, a summary or a player. */
  const hasBody = $derived(detail !== null && (detail.transcript.length > 0 || detail.summary !== null || detail.media !== null));

  const copied = new Flash();
  $effect(() => () => copied.clear());

  async function copySummary() {
    if (detail?.summary && (await store.copy(detail.summary.markdown))) copied.show();
  }
</script>

{#if row}
  <article class="detail">
    <header>
      <div class="titles">
        <h1>{row.title}</h1>
        {#if meta}<p class="meta">{meta}</p>{/if}
      </div>
      <div class="actions">
        {#if detail?.summary}
          <button type="button" onclick={copySummary}>{copied.on ? t("meetings.copied") : t("meetings.summary.copy")}</button>
        {/if}
        {#if detail}<ExportMenu {store} {detail} />{/if}
        <DetailMenu {store} {row} {detail} onrelease={() => player.release()} />
      </div>
    </header>

    {#if detail?.mediaMissing}
      <p class="missing" role="status">{t("meetings.player.missing")}</p>
    {/if}

    {#if row.live}
      <Progress {store} job={row.live} kind={row.kind} {detail} />
    {:else}
      {#if row.error}
        <ErrorCard {store} id={row.id} error={row.error} hasTranscript={(detail?.transcript.length ?? 0) > 0} />
      {/if}
      {#if detail && hasBody}
        <div class="split">
          <div class="left">
            <Player {player} />
            <Transcript
              lines={detail.transcript}
              draft={detail.transcriptIsLive}
              currentTime={player.currentTime}
              playing={player.playing}
              canSeek={player.available}
              canRename={detail.speakers.length > 0}
              onseek={seek}
              onrename={(speaker) => store.openRename(speaker)}
              oncopy={() => store.copy(transcriptText(detail.transcript))}
            />
          </div>
          <div class="right">
            <SummaryPane {store} {detail} canSeek={player.available} onseek={seek} />
          </div>
        </div>
      {/if}
    {/if}
  </article>
{:else}
  <p class="none">{t("meetings.detail.none")}</p>
{/if}

<style>
  /* The article is as high as the window: the transcript and the summary scroll on their own, so that the player stays in sight. When
     the window is too low for them the article scrolls as a whole. */
  .detail {
    display: flex;
    flex-direction: column;
    height: 100%;
    padding: 16px 20px;
    overflow-y: auto;
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 16px;
  }
  .titles {
    flex: 1;
    min-width: 0;
  }
  .actions {
    display: flex;
    flex: none;
    align-items: center;
    gap: 6px;
  }
  h1 {
    margin: 0 0 2px;
    font-size: 18px;
    font-weight: 600;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .meta {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
    user-select: text;
  }
  .missing {
    margin: 0 0 12px;
    padding: 6px 10px;
    color: var(--warn-fg);
    background: var(--warn-bg);
    border-radius: 8px;
  }
  /* The columns can be as narrow as 300 and 280 px: at the narrowest window (960 px) with the list open (280 px) the detail has 640 px
     for them, less its scroll bar, and a gap. They grow in the proportions of 480 to 420 (layout.test.ts adds it up). */
  .split {
    display: grid;
    grid-template-columns: minmax(300px, 480fr) minmax(280px, 420fr);
    grid-template-rows: minmax(0, 1fr);
    flex: 1;
    gap: 16px;
    min-height: 420px;
  }
  .left,
  .right {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .none {
    margin: 0;
    padding: 48px 16px;
    text-align: center;
    color: var(--muted);
  }
</style>
