<script lang="ts">
  import type { Snippet } from "svelte";
  import { trapTab } from "../lib/focus";

  // A dialog over the window, on a backdrop. Esc and a press on the backdrop close it, and Tab stays in it. It is there while it
  // is open: the page that has it renders it only then.
  let {
    labelledby,
    onclose,
    children,
  }: {
    /** The id of the heading that names the dialog. */
    labelledby: string;
    onclose: () => void;
    children: Snippet;
  } = $props();

  let dialog: HTMLElement | undefined = $state();
  /** The press that is going on started on the backdrop itself: a selection that is dragged out of the dialog doesn't close it. */
  let pressedBackdrop = false;

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      onclose();
    } else {
      trapTab(event, dialog);
    }
  }
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="presentation"
  onpointerdown={(event) => (pressedBackdrop = event.target === event.currentTarget)}
  onclick={(event) => {
    const closes = pressedBackdrop && event.target === event.currentTarget;
    pressedBackdrop = false;
    if (closes) onclose();
  }}
>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby={labelledby} bind:this={dialog}>
    {@render children()}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgb(0 0 0 / 0.4);
  }
  .dialog {
    display: flex;
    flex-direction: column;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 32px);
    padding: 14px 16px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.35);
  }
</style>
