<script lang="ts">
  import { tick } from "svelte";
  import { api } from "../lib/api";
  import { t } from "../lib/i18n";
  import LanguageMenu from "./LanguageMenu.svelte";
  import Waveform from "./Waveform.svelte";
  import { errorText, formatTimer, joiner, pillCaption, type PanelState } from "./state";

  // The pill while dictating with no text field to write into: the words so far above, and a capsule to stop from.
  let { panel }: { panel: PanelState } = $props();

  const finishing = $derived(panel.phase === "finishing");
  const done = $derived(panel.committed);
  const live = $derived(panel.partial);
  const empty = $derived(done === "" && live === "");
  const between = $derived(done !== "" && live !== "" ? joiner(panel.language) : "");

  let scroller: HTMLElement | undefined = $state();

  $effect(() => {
    void [done, live];
    void tick().then(() => {
      if (scroller) scroller.scrollTop = scroller.scrollHeight;
    });
  });

  // Return stops and Esc cancels, from wherever the focus is in the window. A button that has the focus takes its own
  // Return, as buttons do, and a menu takes the Esc that closes it before it gets here.
  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      void api.dictationCancel();
    } else if (event.key === "Enter" && !finishing && !(event.target instanceof HTMLButtonElement)) {
      event.preventDefault();
      void api.dictationStop();
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="stack">
  {#if !empty}
    <div class="live">
      <div class="scroll" bind:this={scroller}>
        <p class="text">
          {#if done !== ""}<span>{done.trimEnd()}</span>{/if}{between}{#if live !== ""}<span style:color="var(--muted)">{live.trimStart()}</span>{/if}
        </p>
      </div>
    </div>
  {/if}

  <div class="pill">
    <span class="dot" data-testid="dot" style:opacity={panel.level < 0.04 ? 0.55 : 1}></span>
    <Waveform level={panel.level} />
    <span class="timer">{formatTimer(panel.durationS)}</span>
    <LanguageMenu language={panel.language} height={28} />
    <button type="button" class="stop" disabled={finishing} onclick={() => api.dictationStop()}>
      <span class="square" aria-hidden="true"></span>
      <span class="label">{finishing ? t("panel.finishing") : t("panel.stop")}</span>
    </button>
  </div>

  {#if panel.error}
    <p class="problem" role="alert">{errorText(panel.error)}</p>
  {/if}

  <p class="caption">{pillCaption(panel)}</p>
</div>

<style>
  .stack {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    width: max-content;
    color: var(--ink);
  }

  .live {
    box-sizing: border-box;
    width: 420px;
    padding: 12px 16px;
    border-radius: 14px;
    background: color-mix(in srgb, var(--panel) 94%, transparent);
    box-shadow: 0 0 0 1px var(--line), var(--shadow-pop);
  }

  .scroll {
    max-height: 150px;
    overflow-y: auto;
  }

  .text {
    margin: 0;
    font-size: 15px;
    line-height: 1.4;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .pill {
    display: flex;
    align-items: center;
    gap: 12px;
    height: 50px;
    padding: 0 8px 0 16px;
    border-radius: 999px;
    background: var(--panel);
    box-shadow: 0 0 0 1px var(--line), var(--shadow-pop);
  }

  .dot {
    flex: none;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--record);
  }

  .timer {
    min-width: 2.6em;
    color: var(--muted);
    font-size: 13px;
    font-variant-numeric: tabular-nums;
  }

  .stop {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 34px;
    padding: 0 14px;
    border: 0;
    border-radius: 999px;
    background: var(--accent);
    color: var(--on-accent);
    font: inherit;
    font-weight: 600;
    white-space: nowrap;
    cursor: pointer;
  }

  .stop:disabled {
    opacity: 0.7;
    cursor: default;
  }

  .square {
    width: 10px;
    height: 10px;
    border-radius: 2px;
    background: currentColor;
  }

  .problem {
    box-sizing: border-box;
    width: 420px;
    margin: 0;
    padding: 6px 12px;
    border-radius: 12px;
    background: color-mix(in srgb, var(--panel) 94%, transparent);
    color: var(--warn);
    font-size: 12px;
    line-height: 1.35;
    text-align: center;
  }

  .caption {
    margin: 0;
    padding: 3px 10px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--panel) 85%, transparent);
    font-size: 12px;
    white-space: nowrap;
  }
</style>
