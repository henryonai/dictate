<script lang="ts">
  import { tick, untrack } from "svelte";
  import { formatClock } from "../lib/days";
  import { t } from "../lib/i18n";
  import { needsLibav } from "../lib/player.svelte";
  import type { PlayerController } from "../lib/player.svelte";

  // The video or the audio of a recording. A recording with a picture is a video in a box (small, or large with Enlarge) that has the
  // controls of the system; Hide video puts the box out of sight and the sound goes on, because it is the same element, kept in the page
  // as a transparent pixel (an element that is not drawn at all may be stopped by an engine). One with no picture (an audio file, or a
  // "video" whose picture can't be decoded) is a slim bar: play, position, time and speed.
  // What keeps it playing (the address that changes, the errors, the waits) is the controller's: this only shows its element and
  // gives it to it. It is hidden when there is no media.
  let { player }: { player: PlayerController } = $props();

  /** The speeds of the slim bar: its button goes to the next one up, and round to the first. */
  const SPEEDS = [1, 1.5, 2];
  const nextSpeed = (rate: number) => SPEEDS.find((speed) => speed > rate) ?? SPEEDS[0];

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

  // Hide video and Show video swap the controls: the one that was pressed goes, and the keys go with the one that takes its place.
  let hideButton: HTMLButtonElement | undefined = $state();
  let showButton: HTMLButtonElement | undefined = $state();

  async function hideVideo() {
    player.hideVideo();
    await tick();
    showButton?.focus();
  }

  async function showVideo() {
    player.showVideo();
    await tick();
    hideButton?.focus();
  }

  /** "01:35 / 10:00", or only "01:35" while the length is not known. */
  const where = $derived(
    player.duration > 0 ? `${formatClock(player.currentTime)} / ${formatClock(player.duration)}` : formatClock(player.currentTime),
  );
</script>

{#if player.url !== ""}
  <div class="player">
    {#if libav}<p class="notice" role="status">{t("meetings.player.needs_libav")}</p>{/if}
    {#if player.failed}
      <p class="notice" role="status">{t("meetings.player.failed")}</p>
    {:else}
      {#if player.video}
        <!-- Out of sight, and still the one that plays, when the picture is hidden or there is none. -->
        <video
          bind:this={element}
          class:large={player.showsPicture && player.size === "large"}
          class:away={!player.showsPicture}
          aria-hidden={player.showsPicture ? undefined : "true"}
          controls={player.showsPicture}
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
          <button type="button" class="plain small" bind:this={hideButton} onclick={hideVideo}>{t("meetings.player.hide_video")}</button>
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
            aria-valuetext={where}
            oninput={scrub}
          />
          <!-- The total goes when the bar is narrow (the container rule below): the slider keeps its room. -->
          <span class="time">{formatClock(player.currentTime)}<span class="total">{player.duration > 0 ? ` / ${formatClock(player.duration)}` : ""}</span></span>
          <!-- One button that shows the speed it has and goes to the next one: three of them would not fit a narrow column. -->
          <button
            type="button"
            class="plain speed"
            aria-label={t("meetings.player.speed", { rate: player.rate })}
            title={t("meetings.player.speed", { rate: player.rate })}
            onclick={() => player.setRate(nextSpeed(player.rate))}
          >
            {player.rate}×
          </button>
          {#if player.hasPicture}
            <button
              type="button"
              class="plain icon"
              bind:this={showButton}
              aria-label={t("meetings.player.show_video")}
              title={t("meetings.player.show_video")}
              onclick={showVideo}
            >
              <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true">
                <rect x="1.5" y="3" width="13" height="10" rx="2" />
                <path d="M6.6 5.9v4.2l3.4-2.1z" fill="currentColor" stroke="none" />
              </svg>
            </button>
          {/if}
        </div>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .player {
    position: relative;
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
  /* `cqh` is the height of the column (the left column of the detail is a container of its size): the picture leaves it 200 px for the
     controls and the transcript. A web view that does not know `cqh` keeps the rule above. */
  @supports (height: 1cqh) {
    video.large {
      height: min(70vh, 720px, calc(100cqh - 200px));
    }
  }
  /* Out of sight and still a video that plays: a pixel, see-through, out of the way of the pointer and of the layout. */
  video.away {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
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
  /* The slim bar: one line, 48 px at most, and made to fit the narrowest column of the window (layout.test.ts adds it up): the play
     button, a slider that is never narrower than 80 px, the time, one button for the speed and, when the video is hidden, one for
     the video. It is a container, so that the total of the time can go when it is narrow. */
  .bar {
    container: bar / inline-size;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 40px;
    padding: 0 8px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 8px;
  }
  @container bar (max-width: 330px) {
    .total {
      display: none;
    }
  }
  .toggle,
  .icon {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
  }
  .toggle {
    border-radius: 999px;
  }
  .position {
    flex: 1 1 80px;
    min-width: 80px;
    accent-color: var(--accent);
  }
  .time {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    font-family: var(--mono);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    color: var(--muted);
  }
  .speed {
    flex: none;
    width: 52px;
    padding: 1px 0;
    font-size: 12px;
    text-align: center;
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
