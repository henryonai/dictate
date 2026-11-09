<script lang="ts">
  import { formatDuration } from "../lib/days";
  import { currentLanguage, t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import ErrorCard from "./ErrorCard.svelte";
  import Progress from "./Progress.svelte";

  // The selected recording: the header (its title and where it came from), and then what it is doing or why it stopped.
  // The transcript, the summary, the player and the questions come under these.
  let { store }: { store: MeetingsStore } = $props();

  const row = $derived(store.selectedRow);
  /** The detail, when it is the selected recording's: the store drops it while another one is being read. */
  const detail = $derived(store.detail && store.detail.row.id === store.selectedId ? store.detail : null);

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
</script>

{#if row}
  <article class="detail">
    <header>
      <h1>{row.title}</h1>
      {#if meta}<p class="meta">{meta}</p>{/if}
    </header>

    {#if row.live}
      <Progress {store} job={row.live} kind={row.kind} {detail} />
    {:else if row.error}
      <ErrorCard {store} id={row.id} error={row.error} hasTranscript={(detail?.transcript.length ?? 0) > 0} />
    {/if}
  </article>
{:else}
  <p class="none">{t("meetings.detail.none")}</p>
{/if}

<style>
  .detail {
    padding: 16px 20px;
  }
  header {
    margin-bottom: 16px;
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
  .none {
    margin: 0;
    padding: 48px 16px;
    text-align: center;
    color: var(--muted);
  }
</style>
