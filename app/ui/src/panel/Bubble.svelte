<script lang="ts">
  import { tick } from "svelte";
  import { api } from "../lib/api";
  import { t } from "../lib/i18n";
  import LanguageMenu from "./LanguageMenu.svelte";
  import Waveform from "./Waveform.svelte";
  import { bubbleHint, bubbleTitle, errorText, joiner, type PanelState } from "./state";

  // The bubble by the caret: the words as they are heard, then what the app does with them until they are written.
  let { panel }: { panel: PanelState } = $props();

  const recording = $derived(panel.phase === "recording");
  const working = $derived(panel.phase === "working");
  // While the app works the words are the final text; while it listens they are the final part and the guess for the rest.
  const done = $derived(working ? panel.text : panel.committed);
  const live = $derived(working ? "" : panel.partial);
  const empty = $derived(done === "" && live === "");
  const between = $derived(done !== "" && live !== "" ? joiner(panel.language) : "");
  const title = $derived(bubbleTitle(panel));
  const hint = $derived(bubbleHint(panel));

  let scroller: HTMLElement | undefined = $state();

  // The newest words stay in sight.
  $effect(() => {
    void [done, live];
    void tick().then(() => {
      if (scroller) scroller.scrollTop = scroller.scrollHeight;
    });
  });
</script>

<div class="bubble" class:fit={empty}>
  <div class="header">
    {#if recording}
      <span class="dot" data-testid="dot" style:opacity={panel.level < 0.04 ? 0.55 : 1}></span>
      <span class="slot"><Waveform level={panel.level} scale={0.7} /></span>
    {:else}
      <span class="spinner" role="status" aria-label={title}></span>
    {/if}
    <span class="title">{title}</span>
    <span class="grow"></span>
    {#if recording}
      <LanguageMenu language={panel.language} height={22} />
    {/if}
    <button type="button" class="close" aria-label={t("panel.cancel")} title={t("panel.cancel")} onclick={() => api.dictationCancel()}>
      <svg width="9" height="9" viewBox="0 0 9 9" aria-hidden="true">
        <path d="M1 1l7 7M8 1L1 8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
      </svg>
    </button>
  </div>

  {#if !empty}
    <div class="scroll" bind:this={scroller}>
      <p class="text" style:opacity={working ? 0.7 : 1}>
        {#if done !== ""}<span>{done.trimEnd()}</span>{/if}{between}{#if live !== ""}<span style:color="var(--muted)">{live.trimStart()}</span>{/if}
      </p>
    </div>
  {/if}

  {#if panel.error}
    <div class="problem" role="alert">{errorText(panel.error)}</div>
  {/if}

  {#if hint}
    <div class="hint">{hint}</div>
  {/if}
</div>

<style>
  .bubble {
    box-sizing: border-box;
    width: 400px;
    padding: 9px 12px;
    border: 1px solid var(--line);
    border-radius: 14px;
    background: color-mix(in srgb, var(--panel) 96%, transparent);
    box-shadow: var(--shadow-pop);
    color: var(--ink);
  }

  /* Nothing to show yet but the header: no wider than it needs. */
  .bubble.fit {
    width: max-content;
  }

  .header {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--muted);
    font-size: 11px;
    font-weight: 500;
  }

  .title {
    white-space: nowrap;
  }

  .grow {
    flex: 1;
    min-width: 6px;
  }

  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--record);
  }

  .slot {
    display: flex;
    align-items: center;
    height: 16px;
  }

  .spinner {
    flex: none;
    box-sizing: border-box;
    width: 12px;
    height: 12px;
    border: 1.5px solid var(--line);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .close {
    display: inline-flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: var(--hover);
    color: inherit;
    cursor: pointer;
  }

  .scroll {
    max-height: 120px;
    margin-top: 8px;
    overflow-y: auto;
  }

  .text {
    margin: 0;
    font-size: 14px;
    line-height: 1.45;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .hint {
    margin-top: 8px;
    color: var(--muted);
    font-size: 10px;
  }

  .problem {
    margin-top: 8px;
    color: var(--warn);
    font-size: 11px;
    line-height: 1.35;
  }

  /* A web view before Safari 16.2 (macOS 13.0 has 16.0) does not know color-mix(): the bubble is then solid. */
  @supports not (color: color-mix(in srgb, red, blue)) {
    .bubble {
      background: var(--panel);
    }
  }
</style>
