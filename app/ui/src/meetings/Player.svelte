<script lang="ts">
  import { untrack } from "svelte";
  import { t } from "../lib/i18n";
  import { needsLibav } from "../lib/player.svelte";
  import type { PlayerController } from "../lib/player.svelte";

  // The video or the audio of a recording, with the controls of the system. What keeps it playing (the address that changes, the
  // errors, the waits) is the controller's: this only shows its element and gives it to it. It is hidden when there is no media.
  let { player }: { player: PlayerController } = $props();

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
</script>

{#if player.url !== ""}
  <div class="player">
    {#if libav}<p class="notice" role="status">{t("meetings.player.needs_libav")}</p>{/if}
    {#if player.failed}
      <p class="notice" role="status">{t("meetings.player.failed")}</p>
    {:else if player.video}
      <video bind:this={element} controls preload="metadata" playsinline></video>
    {:else}
      <audio bind:this={element} controls preload="metadata"></audio>
    {/if}
  </div>
{/if}

<style>
  .player {
    flex: none;
    margin-bottom: 10px;
  }
  video {
    display: block;
    width: 100%;
    max-height: 360px;
    aspect-ratio: 16 / 10;
    background: #000;
    border-radius: 8px;
  }
  audio {
    display: block;
    width: 100%;
    height: 44px;
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
