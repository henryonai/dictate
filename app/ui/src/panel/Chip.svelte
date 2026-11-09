<script lang="ts">
  import { api } from "../lib/api";
  import { t } from "../lib/i18n";
  import ChipMenu from "./ChipMenu.svelte";
  import { chipTitle, onMac, type PanelState } from "./state";

  // The chip that stays by the text that was written: what was done to it, and a ⋯ button for what can be done next.
  // It goes by itself after a while; the Rust side keeps the time, and is told when the mouse is over the chip and when
  // the menu is open, because it waits for neither. On a Mac the mouse is the window's to watch (the page of a panel
  // that is never the key window gets no mouse movement), and the Rust side hears it there.
  let { panel }: { panel: PanelState } = $props();

  const title = $derived(chipTitle(panel));
  const warning = $derived(panel.notice !== null || panel.error !== null);
  const writtenSomething = $derived(panel.text !== "");

  let open = $state(false);
  let root: HTMLElement | undefined = $state();

  function setOpen(next: boolean) {
    if (open === next) return;
    open = next;
    api.dictationMenuOpen(next).catch((e) => console.warn("Could not tell that the menu changed:", e));
  }

  const hoverIsMine = !onMac();

  function hover(over: boolean) {
    if (!hoverIsMine) return;
    api.dictationChipHover(over).catch((e) => console.warn("Could not tell where the mouse is:", e));
  }

  // Esc closes the menu and goes no further; a press anywhere else closes it too. Both listen in the capture phase, ahead
  // of everything that listens on the page.
  $effect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      event.preventDefault();
      setOpen(false);
    };
    const onPointer = (event: Event) => {
      if (root && event.target instanceof Node && !root.contains(event.target)) setOpen(false);
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onPointer, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onPointer, true);
    };
  });

  // A chip that goes away with its menu open leaves nothing that waits for the menu to close.
  $effect(() => () => {
    if (open) api.dictationMenuOpen(false).catch(() => {});
  });
</script>

<div class="chip" role="group" aria-label={title} bind:this={root} onmouseenter={() => hover(true)} onmouseleave={() => hover(false)}>
  <span class="mark" class:warning data-mark={warning ? "warning" : "check"} aria-hidden="true">
    {#if warning}
      <svg width="14" height="14" viewBox="0 0 14 14"><circle cx="7" cy="7" r="7" fill="currentColor" /><path d="M7 3.6v4M7 9.6v.1" stroke="black" stroke-width="1.6" stroke-linecap="round" /></svg>
    {:else}
      <svg width="14" height="14" viewBox="0 0 14 14"><circle cx="7" cy="7" r="7" fill="currentColor" /><path d="M4 7.2l2 2 4-4.2" fill="none" stroke="black" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
    {/if}
  </span>
  <span class="title" {title}>{title}</span>
  {#if writtenSomething}
    <button type="button" class="dots" aria-haspopup="menu" aria-expanded={open} aria-label={t("chip.more")} title={t("chip.more")} onclick={() => setOpen(!open)}>
      <svg width="14" height="4" viewBox="0 0 14 4" aria-hidden="true"><circle cx="2" cy="2" r="1.4" fill="currentColor" /><circle cx="7" cy="2" r="1.4" fill="currentColor" /><circle cx="12" cy="2" r="1.4" fill="currentColor" /></svg>
    </button>
  {/if}
  {#if open}
    <!-- Right under the chip with no gap, so that the mouse never leaves it on its way to the menu. -->
    <div class="popup">
      <ChipMenu {panel} onclose={() => setOpen(false)} />
    </div>
  {/if}
</div>

<style>
  .chip {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 6px 0 12px;
    border: 1px solid rgb(255 255 255 / 0.08);
    border-radius: 16px;
    background: rgb(28 28 28 / 0.96);
    box-shadow: 0 6px 12px rgb(0 0 0 / 0.28);
    color: rgb(245 245 245);
    font-size: 12px;
    font-weight: 500;
  }

  .mark {
    display: inline-flex;
    flex: none;
    color: rgb(48 209 88);
  }

  .mark.warning {
    color: rgb(255 159 10);
  }

  .title {
    max-width: 340px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dots {
    display: inline-flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 11px;
    background: rgb(59 59 59);
    color: inherit;
    cursor: pointer;
  }

  .dots[aria-expanded="true"] {
    background: rgb(84 84 84);
  }

  .popup {
    position: absolute;
    top: 100%;
    left: 0;
    z-index: 10;
    padding-top: 6px;
  }
</style>
