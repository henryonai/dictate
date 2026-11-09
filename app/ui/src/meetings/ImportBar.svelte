<script lang="ts">
  import { tick } from "svelte";
  import { trapTab } from "../lib/focus";
  import { t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import LinkForm from "./LinkForm.svelte";

  // The toolbar: the list's toggle, the menu that adds a file or a link, and Settings. (Recording is not part of this window yet.)
  let { store }: { store: MeetingsStore } = $props();

  let menuOpen = $state(false);
  let linkOpen = $state(false);
  let addRoot: HTMLElement | undefined = $state();
  let addButton: HTMLButtonElement | undefined = $state();
  let dialog: HTMLElement | undefined = $state();

  /** The toggle is for a window that has recordings (or a search going on). */
  const hasRecordings = $derived(store.rows.length > 0 || store.query.trim() !== "");

  function closeMenu() {
    menuOpen = false;
  }

  async function chooseFile() {
    closeMenu();
    await store.pickFiles();
  }

  function openLink() {
    closeMenu();
    linkOpen = true;
  }

  async function closeLink() {
    linkOpen = false;
    await tick();
    addButton?.focus();
  }

  function onkeydown(event: KeyboardEvent) {
    if (linkOpen) {
      if (event.key === "Escape") {
        event.preventDefault();
        void closeLink();
      } else {
        trapTab(event, dialog);
      }
    } else if (menuOpen && event.key === "Escape") {
      event.preventDefault();
      closeMenu();
      addButton?.focus();
    }
  }

  function onpointerdown(event: PointerEvent) {
    if (menuOpen && addRoot && event.target instanceof Node && !addRoot.contains(event.target)) closeMenu();
  }
</script>

<svelte:window {onkeydown} {onpointerdown} />

<header class="toolbar" role="toolbar" aria-label={t("meetings.toolbar.label")}>
  {#if hasRecordings}
    <button type="button" aria-expanded={store.sidebarOpen} aria-controls="library-list" onclick={() => store.toggleSidebar()}>
      {store.sidebarOpen ? t("meetings.list.hide") : t("meetings.list.show")}
    </button>
  {/if}
  <span class="spacer"></span>
  <div class="add" bind:this={addRoot}>
    <button
      type="button"
      aria-haspopup="menu"
      aria-expanded={menuOpen}
      bind:this={addButton}
      onclick={() => (menuOpen = !menuOpen)}
    >
      <span aria-hidden="true">+</span> {t("meetings.add")}
    </button>
    {#if menuOpen}
      <div class="menu" role="menu">
        <button type="button" role="menuitem" onclick={chooseFile}>{t("meetings.add.file")}</button>
        <button type="button" role="menuitem" onclick={openLink}>{t("meetings.add.link")}</button>
      </div>
    {/if}
  </div>
  <button type="button" onclick={() => store.openSettings("general")}>{t("meetings.settings")}</button>
</header>

{#if linkOpen}
  <div
    class="backdrop"
    role="presentation"
    onclick={(event) => {
      if (event.target === event.currentTarget) void closeLink();
    }}
  >
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="link-dialog-title" bind:this={dialog}>
      <h2 id="link-dialog-title">{t("meetings.link.title")}</h2>
      <p class="hint">{t("meetings.link.hint")}</p>
      <LinkForm {store} prefill autofocus oncancel={closeLink} ondone={closeLink} />
    </div>
  </div>
{/if}

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .spacer {
    flex: 1;
  }
  .add {
    position: relative;
  }
  .menu {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    z-index: 10;
    display: flex;
    flex-direction: column;
    min-width: 260px;
    padding: 4px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 6px 16px rgb(0 0 0 / 0.25);
  }
  .menu button {
    padding: 6px 10px;
    text-align: left;
    background: transparent;
    border-color: transparent;
  }
  .menu button:hover,
  .menu button:focus-visible {
    background: var(--accent-soft);
  }
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
    width: min(460px, calc(100vw - 32px));
    padding: 14px 16px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.35);
  }
  .dialog h2 {
    margin: 0 0 4px;
    font-size: 14px;
    letter-spacing: normal;
    text-transform: none;
    color: var(--fg);
  }
</style>
