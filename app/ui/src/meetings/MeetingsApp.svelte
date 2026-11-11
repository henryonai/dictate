<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorText, takeOpenRecorder } from "../lib/api";
  import { t } from "../lib/i18n";
  import { MeetingsStore } from "../lib/meetings-store.svelte";
  import Detail from "./Detail.svelte";
  import Empty from "./Empty.svelte";
  import ImportBar from "./ImportBar.svelte";
  import List from "./List.svelte";
  import LogSheet from "./LogSheet.svelte";
  import RecordPicker from "./RecordPicker.svelte";

  // The meetings window: the library of recordings with what is going on in each, and the ways to add more (a file, a link, a
  // drop, a paste).
  const store = new MeetingsStore();

  /** Files are being dragged over the window. */
  let dragging = $state(false);

  onMount(() => {
    // A window that was made for the picker has the ask for it in a global (the page was not there to hear an event): it is read
    // first, and taken away. The asks that come later are events, heard after. An ask is the same ask either way: the picker is
    // open once.
    const asked = takeOpenRecorder();
    if (asked) store.openRecorder(asked.error);
    // The language is the settings': `start` reads it (with what is ready) and the store makes the window speak it.
    // A failure is said in the notice: a window that can't start must not be a blank one.
    store.start().catch((e: unknown) => store.notify(errorText(e)));
    const stopDrops = api.onFileDrop((event) => {
      dragging = event.type === "enter";
      if (event.type === "drop") void store.importFiles(event.paths);
    }).catch((e: unknown) => {
      store.notify(errorText(e));
      return () => {};
    });
    const stopAsks = api.onOpenRecorder((open) => store.openRecorder(open.error)).catch((e: unknown) => {
      store.notify(errorText(e));
      return () => {};
    });
    return () => {
      store.stop();
      void stopDrops.then((stop) => stop());
      void stopAsks.then((stop) => stop());
    };
  });

  /**
   * A paste into a text field is text; anywhere else, a link that is pasted is imported, unless a dialog is open: what is pasted is
   * for the dialog (or for nothing), not for the library behind it.
   */
  function onpaste(event: ClipboardEvent) {
    if (store.modalOpen) return;
    const target = event.target;
    if (target instanceof HTMLElement && (target.isContentEditable || /^(input|textarea|select)$/i.test(target.tagName))) return;
    const pasted = event.clipboardData?.getData("text")?.trim() ?? "";
    if (!/^https?:\/\//i.test(pasted)) return;
    event.preventDefault();
    void importPasted(pasted);
  }

  async function importPasted(url: string) {
    const problem = await store.importLink(url);
    if (problem) store.notify(problem);
  }
</script>

<svelte:document {onpaste} />

<main class="meetings">
  <ImportBar {store} />

  {#if store.notice}
    <div class="notice" role="alert">
      <span>{store.notice}</span>
      <button type="button" onclick={() => store.dismissNotice()}>{t("meetings.notice.dismiss")}</button>
    </div>
  {/if}

  {#if !store.loaded}
    <!-- The list is being read. -->
  {:else if store.items.length === 0 && store.query.trim() === ""}
    <div class="scroll"><Empty {store} /></div>
  {:else}
    <div class="body">
      {#if store.sidebarOpen}
        <aside id="library-list"><List {store} /></aside>
      {/if}
      <section class="detail"><Detail {store} /></section>
    </div>
  {/if}

  <LogSheet {store} />

  {#if store.recorder}
    <RecordPicker {store} />
  {/if}

  {#if dragging}
    <div class="drop-overlay" role="presentation">{t("meetings.drop.overlay")}</div>
  {/if}
</main>

<style>
  .meetings {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .notice {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 14px;
    color: var(--warn-fg);
    background: var(--warn-bg);
    overflow-wrap: anywhere;
  }
  .notice button {
    flex: none;
  }
  .body {
    display: flex;
    flex: 1;
    min-height: 0;
  }
  aside {
    flex: none;
    width: 280px;
    min-width: 240px;
    max-width: 360px;
    background: var(--surface);
    border-right: 1px solid var(--border);
  }
  .detail,
  .scroll {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow-y: auto;
  }
  .drop-overlay {
    position: fixed;
    inset: 8px;
    z-index: 30;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 16px;
    font-weight: 600;
    color: var(--accent);
    background: color-mix(in srgb, var(--bg) 85%, transparent);
    border: 3px dashed var(--accent);
    border-radius: 14px;
    pointer-events: none;
  }
</style>
