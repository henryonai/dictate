<script lang="ts">
  import { t } from "../lib/i18n";
  import Inline from "./Inline.svelte";

  // The overview of a summary, the paragraph that says what the recording was about: cut at three lines, with "Show more" when it is
  // longer, so that the key points under it are in sight. Its stamps still seek.
  let { text, onseek }: { text: string; onseek?: (seconds: number) => void } = $props();

  const id = $props.id();
  let box: HTMLElement | undefined = $state();
  let expanded = $state(false);
  /** The text is taller than its three lines, so there is more to show. Measured while it is cut (open, it has no cut to measure). */
  let longer = $state(false);

  // The box is measured when it is shown, when its text changes and when its width does (a narrower column makes more lines).
  $effect(() => {
    void text;
    const element = box;
    if (!element || expanded) return;
    const measure = () => {
      longer = element.scrollHeight > element.clientHeight + 1;
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const watch = new ResizeObserver(measure);
    watch.observe(element);
    return () => watch.disconnect();
  });
</script>

<div class="overview" class:clamped={!expanded} {id} bind:this={box}><Inline {text} {onseek} /></div>
{#if longer || expanded}
  <button type="button" class="plain more" aria-expanded={expanded} aria-controls={id} onclick={() => (expanded = !expanded)}>
    {expanded ? t("meetings.summary.less") : t("meetings.summary.more")}
  </button>
{/if}

<style>
  /* Three lines and no more: the rest is cut (`-webkit-line-clamp` is the way of Safari, of the WebView2 and of WebKitGTK, which all
     take the plain `line-clamp` too). */
  .overview.clamped {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    overflow: hidden;
  }
  .more {
    margin-top: 2px;
    padding: 1px 6px;
    font-size: 12px;
    color: var(--accent);
  }
</style>
