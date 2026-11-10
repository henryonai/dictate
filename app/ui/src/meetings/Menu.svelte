<script lang="ts">
  import { tick } from "svelte";
  import type { Snippet } from "svelte";

  // A button that shows a short list of choices under it: a disclosure with plain buttons (as the toolbar's Add menu is), not a
  // `role="menu"`. A choice closes it, gives the focus back to its button, and then does what it is for; Esc and a press outside close it.
  let {
    text,
    label,
    glyph = false,
    disabled = false,
    children,
  }: {
    /** What the button shows. */
    text: string;
    /** What the list is called (and what a button that shows only a glyph is called). */
    label: string;
    /** The button shows a symbol: its label is the name of it. */
    glyph?: boolean;
    disabled?: boolean;
    /** The choices. `pick` takes what a choice does: it closes the list first. */
    children: Snippet<[pick: (action: () => unknown) => Promise<void>]>;
  } = $props();

  const id = $props.id();
  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  let trigger: HTMLButtonElement | undefined = $state();

  async function pick(action: () => unknown): Promise<void> {
    open = false;
    // The focus goes back to the button before the action runs: a sheet that the action opens remembers where the focus was.
    await tick();
    trigger?.focus();
    await action();
  }

  async function close(): Promise<void> {
    open = false;
    await tick();
    trigger?.focus();
  }

  function onkeydown(event: KeyboardEvent) {
    if (open && event.key === "Escape") {
      event.preventDefault();
      void close();
    }
  }

  function onpointerdown(event: PointerEvent) {
    if (open && root && event.target instanceof Node && !root.contains(event.target)) open = false;
  }
</script>

<svelte:window {onkeydown} {onpointerdown} />

<div class="menu" bind:this={root}>
  <button
    type="button"
    bind:this={trigger}
    {disabled}
    aria-label={glyph ? label : undefined}
    title={glyph ? label : undefined}
    aria-expanded={open}
    aria-controls={open ? id : undefined}
    onclick={() => (open = !open)}
  >
    {text}
  </button>
  {#if open}
    <div class="choices" {id} role="group" aria-label={label}>
      {@render children(pick)}
    </div>
  {/if}
</div>

<style>
  .menu {
    position: relative;
  }
  .choices {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    z-index: 10;
    display: flex;
    flex-direction: column;
    min-width: 280px;
    padding: 4px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 6px 16px rgb(0 0 0 / 0.25);
  }
  .choices :global(button) {
    padding: 6px 10px;
    text-align: left;
    background: transparent;
    border-color: transparent;
  }
  .choices :global(button:hover:not(:disabled)),
  .choices :global(button:focus-visible) {
    background: var(--accent-soft);
  }
  .choices :global(button.danger) {
    color: var(--danger);
  }
  .choices :global(hr) {
    width: 100%;
    margin: 4px 0;
    border: 0;
    border-top: 1px solid var(--border);
  }
</style>
