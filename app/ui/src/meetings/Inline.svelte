<script lang="ts">
  import { renderInline } from "../lib/markdown";

  // A text of a summary or an answer, with its few marks (bold, lists) and its time stamps as links that seek.
  //
  // The `{@html}` below is the only one in the window, and what it is given is only the output of `renderInline`, which escapes the
  // text first and makes no tag but `<strong>` and `<a href="#" data-seek="<seconds>">`. Never give it anything else.
  let {
    text,
    onseek,
    inline = false,
  }: {
    text: string;
    onseek?: (seconds: number) => void;
    /** Shown in a line of its own text (inside a label), as a `<span>`; else as a block, a `<div>`. */
    inline?: boolean;
  } = $props();

  const html = $derived(renderInline(text));
  let box: HTMLElement | undefined = $state();

  // One listener for all the links, on the box: a click (or Enter on a focused link) on a stamp seeks, and never follows the "#".
  $effect(() => {
    const element = box;
    if (!element) return;
    const seek = (event: MouseEvent) => {
      const link = event.target instanceof Element ? event.target.closest<HTMLElement>("a[data-seek]") : null;
      if (!link || !element.contains(link)) return;
      event.preventDefault();
      const seconds = Number(link.dataset.seek);
      if (onseek && Number.isFinite(seconds)) onseek(seconds);
    };
    element.addEventListener("click", seek);
    return () => element.removeEventListener("click", seek);
  });

  // Without a player a stamp has nothing to seek: it is shown as text, and the keyboard passes it by.
  $effect(() => {
    void html;
    const inert = !onseek;
    for (const link of box?.querySelectorAll<HTMLElement>("a[data-seek]") ?? []) {
      if (inert) {
        link.setAttribute("data-inert", "");
        link.setAttribute("aria-disabled", "true");
        link.setAttribute("tabindex", "-1");
      } else {
        link.removeAttribute("data-inert");
        link.removeAttribute("aria-disabled");
        link.removeAttribute("tabindex");
      }
    }
  });
</script>

<svelte:element this={inline ? "span" : "div"} class="inline" bind:this={box}>{@html html}</svelte:element>

<style>
  .inline {
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    user-select: text;
  }
  .inline :global(a[data-seek]) {
    font-variant-numeric: tabular-nums;
  }
  .inline :global(a[data-inert]) {
    color: inherit;
    text-decoration: none;
    pointer-events: none;
  }
</style>
