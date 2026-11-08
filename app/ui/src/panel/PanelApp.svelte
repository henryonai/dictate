<script lang="ts">
  import { tick } from "svelte";
  import { api } from "../lib/api";
  import { setLanguage } from "../lib/i18n";
  import Bubble from "./Bubble.svelte";
  import Card from "./Card.svelte";
  import Chip from "./Chip.svelte";
  import Pill from "./Pill.svelte";
  import { HIDDEN_STATE, isPanelView, VIEWS_OF, type PanelState } from "./state";

  // One of the two panel windows. `view` is what the page was opened with (`?view=bubble` or `?view=card`) and is the
  // name of the window; both windows hear every state, and each draws only the shapes that are its own.
  let { view }: { view: string | null } = $props();

  const known = $derived(isPanelView(view));
  let current = $state<PanelState>(HIDDEN_STATE);
  const shown = $derived(isPanelView(view) && VIEWS_OF[view].includes(current.view));

  let stage: HTMLElement | undefined = $state();
  /** The size last told to the Rust side, so that the same size is told once. */
  let told = { width: 0, height: 0 };

  // The words follow the language of the settings. It is asked when the panel goes from hidden to shown, and the state
  // that shows it waits for the answer: the window is blank until then, and never drawn in the language of the showing
  // before. States that come meanwhile wait behind it, so that they are applied in the order they were sent.
  async function refreshLanguage() {
    try {
      setLanguage(await api.panelLanguage());
    } catch (e) {
      console.warn("Could not ask for the language:", e);
    }
  }

  let applied: Promise<void> = Promise.resolve();

  function apply(next: PanelState) {
    applied = applied.then(async () => {
      if (current.view === "hidden" && next.view !== "hidden") await refreshLanguage();
      current = next;
    });
  }

  $effect(() => {
    if (!known) return;
    let stopped = false;
    let stop: (() => void) | undefined;
    api
      .onPanelState(apply)
      .then((unlisten) => {
        if (stopped) unlisten();
        else stop = unlisten;
      })
      .catch((e) => console.warn("Could not listen for the panel state:", e));
    return () => {
      stopped = true;
      stop?.();
    };
  });

  /**
   * What the page needs in CSS pixels: the stage, and everything that hangs out of it (a menu opens beyond the edge of
   * the bubble). Rounded up, so that the window is never a fraction too small.
   */
  function measure() {
    if (!stage || !isPanelView(view)) return;
    const box = stage.getBoundingClientRect();
    let right = box.right;
    let bottom = box.bottom;
    for (const extra of stage.querySelectorAll("[data-extent]")) {
      const rect = extra.getBoundingClientRect();
      right = Math.max(right, rect.right);
      bottom = Math.max(bottom, rect.bottom);
    }
    const width = Math.ceil(right - box.left);
    const height = Math.ceil(bottom - box.top);
    if (width <= 0 || height <= 0) return;
    if (width === told.width && height === told.height) return;
    told = { width, height };
    api.panelResized(view, width, height).catch((e) => console.warn("Could not tell the size:", e));
  }

  // Size changes are told to the Rust side, which sizes the window and puts it where it belongs.
  $effect(() => {
    if (!stage) return;
    const resizes = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(() => measure());
    resizes?.observe(stage);
    // A menu that opens or closes adds or removes elements: the stage itself doesn't change size.
    const changes = new MutationObserver(() => measure());
    changes.observe(stage, { childList: true, subtree: true });
    void tick().then(measure);
    return () => {
      resizes?.disconnect();
      changes.disconnect();
      // The window is hidden or shows something else next: the next size is told, whatever it is.
      told = { width: 0, height: 0 };
    };
  });

  // New words make a taller bubble even when the stage isn't observed (a test, or a page that hasn't laid out yet).
  $effect(() => {
    void [current.committed, current.partial, current.text];
    void tick().then(measure);
  });
</script>

{#if shown}
  <div class="stage" bind:this={stage}>
    {#if current.view === "bubble"}
      <Bubble panel={current} />
    {:else if current.view === "chip"}
      <Chip panel={current} />
    {:else if current.view === "pill"}
      <Pill panel={current} />
    {:else if current.view === "card"}
      <Card panel={current} />
    {/if}
  </div>
{/if}

<style>
  /* The padding is room for the shadows of what is drawn: the window is transparent. */
  .stage {
    box-sizing: border-box;
    width: max-content;
    padding: 14px;
  }
</style>
