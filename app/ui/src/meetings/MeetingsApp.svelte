<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { setLanguage, t } from "../lib/i18n";
  import { MeetingsStore } from "../lib/meetings-store.svelte";
  import Detail from "./Detail.svelte";
  import Empty from "./Empty.svelte";
  import ImportBar from "./ImportBar.svelte";
  import List from "./List.svelte";
  import LogSheet from "./LogSheet.svelte";

  // The meetings window: the library of recordings with what is going on in each, and the ways to add more (a file, a link, a
  // drop, a paste).
  const store = new MeetingsStore();

  /** Files are being dragged over the window. */
  let dragging = $state(false);

  onMount(() => {
    // This window can't read the settings (they are Settings' own), so it speaks the language of the system, which is what the
    // setting "same as the system" means; and it tells the page which that is.
    setLanguage("system");
    void store.start();
    const stopDrops = api.onFileDrop((event) => {
      dragging = event.type === "enter";
      if (event.type === "drop") void store.importFiles(event.paths);
    });
    return () => {
      store.stop();
      void stopDrops.then((stop) => stop()).catch(() => {});
    };
  });

  /** A paste into a text field is text; anywhere else, a link that is pasted is imported. */
  function onpaste(event: ClipboardEvent) {
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
    <div class="notice" role="status">
      <span>{store.notice}</span>
      <button type="button" onclick={() => store.dismissNotice()}>{t("meetings.notice.dismiss")}</button>
    </div>
  {/if}

  {#if !store.loaded}
    <!-- The list is being read. -->
  {:else if store.rows.length === 0 && store.query.trim() === ""}
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
