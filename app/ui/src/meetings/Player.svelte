<script lang="ts">
  import { untrack } from "svelte";
  import { formatClock } from "../lib/days";
  import { t } from "../lib/i18n";
  import { needsLibav } from "../lib/player.svelte";
  import type { PlayerController } from "../lib/player.svelte";

  // The video or the audio of a recording. A recording with a picture is a video in a box (small, or large with Enlarge) that has the
  // controls of the system; Hide video takes the box away and the sound goes on, because it is the same element, only not drawn. One
  // with no picture (an audio file, or a "video" whose picture can't be decoded) is a slim bar: play, position, time and speed.
  // What keeps it playing (the address that changes, the errors, the waits) is the controller's: this only shows its element and
  // gives it to it. It is hidden when there is no media.
  let { player }: { player: PlayerController } = $props();

  /** The speeds of the slim bar. */
  const SPEEDS = [1, 1.5, 2];

  let element: HTMLMediaElement | undefined = $state();
  /** A system that can't decode the audio of a recording says which package it lacks. Asked once: the system doesn't change. */
  const libav = needsLibav();

  // The effect depends on the element and on nothing else: `attach` reads the player's address, and an effect that followed it would
  // attach again (and give the element its address again) at each swap, which the player does by itself.
  $effect(() => {
    const shown = element;
    if (!shown) return;
    return untrack(() => player.attach(shown));
  });

  function scrub(event: Event & { currentTarget: HTMLInputElement }) {
    player.scrub(Number(event.currentTarget.value));
  }
</script>

{#if player.url !== ""}
  <div class="player">
    {#if libav}<p class="notice" role="status">{t("meetings.player.needs_libav")}</p>{/if}
    {#if player.failed}
      <p class="notice" role="status">{t("meetings.player.failed")}</p>
    {:else}
      {#if player.video}
        <!-- Not drawn, and still the one that plays, when the picture is hidden or there is none. -->
        <video
          bind:this={element}
          class:large={player.size === "large"}
          controls={player.showsPicture}
          hidden={!player.showsPicture}
          preload="metadata"
          playsinline
        ></video>
      {:else}
        <audio bind:this={element} preload="metadata"></audio>
      {/if}

      {#if player.showsPicture}
        <div class="tools">
          <button type="button" class="plain small" onclick={() => player.toggleLarge()}>
            {player.size === "large" ? t("meetings.player.shrink") : t("meetings.player.enlarge")}
          </button>
          <button type="button" class="plain small" onclick={() => player.hideVideo()}>{t("meetings.player.hide_video")}</button>
        </div>
      {:else}
        <div class="bar">
          <button
            type="button"
            class="toggle"
            aria-label={player.playing ? t("meetings.player.pause") : t("meetings.player.play")}
            onclick={() => player.toggle()}
          >
            <svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor" aria-hidden="true">
              {#if player.playing}
                <rect x="3" y="2" width="3.4" height="12" rx="0.8" />
                <rect x="9.6" y="2" width="3.4" height="12" rx="0.8" />
              {:else}
                <path d="M4 2.2v11.6a.6.6 0 0 0 .92.5l9-5.8a.6.6 0 0 0 0-1L4.92 1.7A.6.6 0 0 0 4 2.2z" />
              {/if}
            </svg>
          </button>
          <input
            type="range"
            class="position"
            min="0"
            max={player.duration}
            step="any"
            value={player.currentTime}
            disabled={!player.available || player.duration === 0}
            aria-label={t("meetings.player.position")}
            aria-valuetext={formatClock(player.currentTime)}
            oninput={scrub}
          />
          <span class="time">{formatClock(player.currentTime)} / {formatClock(player.duration)}</span>
          <div class="speeds" role="group" aria-label={t("meetings.player.speed")}>
            {#each SPEEDS as speed (speed)}
              <button type="button" class="plain speed" aria-pressed={player.rate === speed} onclick={() => player.setRate(speed)}>
                {speed}×
              </button>
            {/each}
          </div>
          {#if player.hasPicture}
            <button type="button" class="plain small" onclick={() => player.showVideo()}>{t("meetings.player.show_video")}</button>
          {/if}
        </div>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .player {
    flex: none;
    margin-bottom: 10px;
  }
  /* About a third of the column, and never more than 360 px; Enlarge takes it to 70 % of the window, 720 px at most. */
  video {
    display: block;
    width: 100%;
    height: min(33vh, 360px);
    background: var(--video-bg);
    border-radius: 8px;
  }
  video.large {
    height: min(70vh, 720px);
  }
  .tools {
    display: flex;
    justify-content: flex-end;
    gap: 4px;
    margin-top: 4px;
  }
  .small {
    padding: 1px 8px;
    font-size: 12px;
  }
  /* The slim bar: one line, 48 px at most. */
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 40px;
    padding: 0 10px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 8px;
  }
  .toggle {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border-radius: 999px;
  }
  .position {
    flex: 1 1 auto;
    min-width: 0;
    accent-color: var(--accent);
  }
  .time {
    flex: none;
    font-family: var(--mono);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    color: var(--muted);
  }
  .speeds {
    display: flex;
    flex: none;
    gap: 2px;
  }
  .speed {
    padding: 1px 6px;
    font-size: 12px;
  }
  .speed[aria-pressed="true"] {
    color: var(--accent);
    background: var(--selected);
  }
  .notice {
    margin: 0 0 6px;
    padding: 6px 10px;
    color: var(--warn-fg);
    background: var(--warn-bg);
    border-radius: 8px;
    overflow-wrap: anywhere;
  }
</style>
