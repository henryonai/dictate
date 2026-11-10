<script lang="ts">
  import type { LineView } from "../lib/api";
  import { Flash } from "../lib/flash.svelte";
  import { t } from "../lib/i18n";

  // The transcript of a recording: a line for each paragraph, with its stamp (which seeks the player) and its speaker (which asks to
  // rename that speaker). While the player plays, the line that is being said is marked and kept in the middle.
  let {
    lines,
    draft,
    currentTime,
    playing,
    canSeek,
    canRename,
    onseek,
    onrename,
    oncopy,
  }: {
    lines: LineView[];
    /** The lines are the rougher live captions of a screen recording. */
    draft: boolean;
    /** Where the player is, in seconds. */
    currentTime: number;
    playing: boolean;
    /** There is a player to seek. */
    canSeek: boolean;
    /** The speakers can be renamed: there are some, and nothing is running on the recording. */
    canRename: boolean;
    onseek: (seconds: number) => void;
    /** `speaker` is the one whose name was clicked; none for the speakers as a whole. */
    onrename: (speaker?: string) => void;
    /** Copies the transcript; says whether it did. */
    oncopy: () => Promise<boolean>;
  } = $props();

  /** A line is the current one a little before it begins, as the person hears it start. */
  const LEAD_S = 0.3;

  /** The last line that has begun. Only while the player plays: a paused player marks nothing. */
  const current = $derived.by(() => {
    if (!playing) return -1;
    for (let index = lines.length - 1; index >= 0; index -= 1) {
      if (lines[index].start <= currentTime + LEAD_S) return index;
    }
    return -1;
  });

  let list: HTMLElement | undefined = $state();

  // The current line goes to the middle of the list when it changes, not on each reading of the position that leaves it where it is.
  $effect(() => {
    if (current < 0) return;
    const quiet = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    list?.querySelector<HTMLElement>(`[data-line="${current}"]`)?.scrollIntoView({ block: "center", behavior: quiet ? "auto" : "smooth" });
  });

  const copied = new Flash();
  $effect(() => () => copied.clear());

  async function copy() {
    if (await oncopy()) copied.show();
  }
</script>

<section class="transcript" aria-labelledby="transcript-title">
  <header>
    <h2 id="transcript-title">{t("meetings.transcript")}</h2>
    {#if draft}<span class="caption">{t("meetings.transcript.draft")}</span>{/if}
    {#if canSeek}<span class="caption">{t("meetings.transcript.follows")}</span>{/if}
    <span class="spacer"></span>
    {#if canRename}
      <button type="button" title={t("meetings.transcript.rename_help")} onclick={() => onrename()}>{t("meetings.transcript.rename")}</button>
    {/if}
    {#if lines.length > 0}
      <button type="button" onclick={copy}>{copied.on ? t("meetings.copied") : t("meetings.transcript.copy")}</button>
    {/if}
  </header>

  <div class="list" bind:this={list}>
    {#if lines.length === 0}
      <p class="empty">{t("meetings.transcript.empty")}</p>
    {:else}
      {#each lines as line, index (index)}
        <p class="line" class:current={index === current} data-line={index} aria-current={index === current ? "true" : undefined}>
          {#if line.stamp}
            <button type="button" class="stamp" disabled={!canSeek} title={t("meetings.transcript.jump", { stamp: line.stamp })} onclick={() => onseek(line.start)}>{line.stamp}</button>
          {/if}
          {#if line.speaker}
            {#if canRename}
              <button type="button" class="speaker" title={t("meetings.transcript.rename_one", { name: line.speaker })} onclick={() => onrename(line.speaker ?? undefined)}>{line.speaker}</button>
            {:else}
              <strong class="speaker">{line.speaker}</strong>
            {/if}
            <span class="dot" aria-hidden="true">·</span>
          {/if}
          <span class="text">{line.text}</span>
        </p>
      {/each}
    {/if}
  </div>
</section>

<style>
  .transcript {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 1;
  }
  header {
    display: flex;
    flex: none;
    align-items: center;
    gap: 8px;
    margin-bottom: 6px;
  }
  h2 {
    margin: 0;
  }
  .caption {
    font-size: 12px;
    color: var(--muted);
  }
  .spacer {
    flex: 1;
  }
  header button {
    padding: 2px 8px;
    font-size: 12px;
  }
  .list {
    flex: 1;
    min-height: 0;
    padding: 6px 4px;
    overflow-y: auto;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
  }
  .empty {
    margin: 0;
    padding: 12px 10px;
    color: var(--muted);
  }
  .line {
    margin: 0;
    padding: 4px 8px;
    border-radius: 6px;
    overflow-wrap: anywhere;
  }
  .line.current {
    background: color-mix(in srgb, #ffd60a 22%, transparent);
  }
  .stamp,
  .speaker {
    padding: 0;
    font: inherit;
    background: transparent;
    border: 0;
    border-radius: 0;
  }
  .stamp {
    margin-right: 6px;
    font-variant-numeric: tabular-nums;
    color: var(--accent);
    text-decoration: underline;
  }
  .stamp:disabled {
    color: var(--muted);
    text-decoration: none;
  }
  .speaker {
    font-weight: 600;
  }
  button.speaker:hover {
    text-decoration: underline;
  }
  .dot {
    margin: 0 2px;
    color: var(--muted);
  }
  .text {
    user-select: text;
  }
</style>
