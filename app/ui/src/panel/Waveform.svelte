<script lang="ts">
  import { barHeights } from "./state";

  // `scale` squeezes it vertically: the bubble draws it smaller than the pill does.
  let { level, scale = 1 }: { level: number; scale?: number } = $props();

  const heights = $derived(barHeights(level));
</script>

<!-- Seven bars that follow the loudness: decoration, so a screen reader skips it. -->
<div class="wave" aria-hidden="true" style:transform={scale === 1 ? undefined : `scaleY(${scale})`}>
  {#each heights as height, i (i)}
    <span class="bar" style:width="3px" style:height="{Math.round(height * 100) / 100}px"></span>
  {/each}
</div>

<style>
  .wave {
    display: flex;
    align-items: center;
    gap: 3px;
    height: 24px;
    flex: none;
  }

  .bar {
    border-radius: 2px;
    background: currentColor;
    transition: height 80ms linear;
  }
</style>
