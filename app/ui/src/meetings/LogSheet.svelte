<script lang="ts">
  import { api, errorText } from "../lib/api";
  import { trapTab } from "../lib/focus";
  import { t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";

  // The log of a recording, in a sheet over the window. It is open while the store has a recording whose log to show.
  let { store }: { store: MeetingsStore } = $props();

  const id = $derived(store.logFor);
  const title = $derived.by(() => {
    if (id === null) return "";
    return store.items.find((row) => row.id === id)?.title ?? (store.detail?.row.id === id ? store.detail.row.title : id);
  });

  /** The text of the log; null while it is read. */
  let text = $state<string | null>(null);
  let failure = $state("");
  let closeButton: HTMLButtonElement | undefined = $state();
  let sheet: HTMLElement | undefined = $state();

  $effect(() => {
    const open = id;
    if (open === null) return;
    text = null;
    failure = "";
    let current = true;
    api.meetingLog(open).then(
      (log) => {
        if (current) text = log;
      },
      (e) => {
        if (current) failure = errorText(e);
      },
    );
    return () => {
      current = false;
    };
  });

  // The sheet takes the focus while it is open, and gives it back to what had it.
  $effect(() => {
    if (id === null) return;
    const before = document.activeElement;
    closeButton?.focus();
    return () => {
      if (before instanceof HTMLElement) before.focus();
    };
  });

  function onkeydown(event: KeyboardEvent) {
    if (id === null) return;
    if (event.key === "Escape") {
      event.preventDefault();
      store.closeLog();
    } else {
      trapTab(event, sheet);
    }
  }
</script>

<svelte:window {onkeydown} />

{#if id !== null}
  <div
    class="backdrop"
    role="presentation"
    onclick={(event) => {
      if (event.target === event.currentTarget) store.closeLog();
    }}
  >
    <div class="sheet" role="dialog" aria-modal="true" aria-labelledby="log-title" bind:this={sheet}>
      <h2 id="log-title">{t("meetings.log.title", { title })}</h2>
      <div class="body">
        {#if failure}
          <p class="error" role="alert">{failure}</p>
        {:else if text === null}
          <!-- Being read. -->
        {:else if text.trim() === ""}
          <p class="hint">{t("meetings.log.empty")}</p>
        {:else}
          <pre>{text}</pre>
        {/if}
      </div>
      <div class="actions">
        <button type="button" disabled={!text || text.trim() === ""} onclick={() => text && store.copy(text)}>{t("meetings.log.copy")}</button>
        <button type="button" class="primary" bind:this={closeButton} onclick={() => store.closeLog()}>{t("meetings.log.close")}</button>
      </div>
    </div>
  </div>
{/if}

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
  .sheet {
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: min(720px, calc(100vw - 32px));
    height: min(480px, calc(100vh - 32px));
    padding: 14px 16px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.35);
  }
  h2 {
    margin: 0;
    font-size: 14px;
    letter-spacing: normal;
    text-transform: none;
    color: var(--fg);
    overflow-wrap: anywhere;
  }
  .body {
    flex: 1;
    min-height: 0;
    padding: 8px 10px;
    overflow: auto;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  pre {
    margin: 0;
    font: 12px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
