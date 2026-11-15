<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { MigrationItem } from "../lib/api";
  import { t } from "../lib/i18n";
  import { SettingsStore } from "../lib/settings-store.svelte";
  import Accounts from "./Accounts.svelte";
  import Ai from "./Ai.svelte";
  import General from "./General.svelte";
  import Speech from "./Speech.svelte";

  const TABS = [
    { id: "general", title: "tabs.general", page: General },
    { id: "speech", title: "tabs.speech", page: Speech },
    { id: "ai", title: "tabs.ai", page: Ai },
    { id: "accounts", title: "tabs.accounts", page: Accounts },
  ] as const;

  const store = new SettingsStore();

  // The mark that the copy is done is not an item that is copied: it is written last, and only when every item was copied. When it is
  // the one thing that failed, the copy is complete, and the note says so in a sentence of its own, not as an item that could not be copied.
  const MARKER = "migrated-from-dictate";

  // The items of the note about the copy from Dictate, in the words of the window: a name for each (the Rust side's own name for an
  // item this window has no name for), and the two kinds of item in a sentence each, as only the ones that have a copy of Dictate's
  // to read are read from it.
  const itemName = (item: string) => {
    const key = `migration.item.${item}`;
    const name = t(key);
    return name === key ? item : name;
  };
  const itemList = (items: MigrationItem[]) => items.map((entry) => `${itemName(entry.item)} (${entry.reason})`).join("; ");
  let tab = $state<(typeof TABS)[number]["id"]>("general");

  onMount(() => {
    void store.load();
    // The desktop may answer for the shortcut after the window opened.
    const stopWatching = store.watchHotkey();
    // The language menu of the dictation chip changes the settings without this window.
    const stopWatchingSettings = store.watchSettings();
    // The meetings window sends people here, to the page where an API key or a model is set up.
    const stopWatchingTabs = api.onSettingsTab((id) => {
      if (TABS.some((entry) => entry.id === id)) tab = id;
    });
    return () => {
      void stopWatching.then((stop) => stop());
      void stopWatchingSettings.then((stop) => stop());
      void stopWatchingTabs.then((stop) => stop());
    };
  });
</script>

<div class="settings">
  {#if store.loadWarning}
    <div class="banner warning" role="alert">
      <span>
        <strong>{t("settings.warning_lead")}</strong>
        {store.loadWarning}
      </span>
      <button type="button" onclick={() => store.dismissWarning()}>{t("settings.dismiss")}</button>
    </div>
  {/if}
  {#if store.migrationNote}
    {@const items = store.migrationNote.filter((entry) => entry.item !== MARKER)}
    {@const marker = store.migrationNote.find((entry) => entry.item === MARKER)}
    {@const used = items.filter((entry) => entry.fallback)}
    {@const unused = items.filter((entry) => !entry.fallback)}
    <div class="banner warning" role="alert">
      <span>
        {#if marker && items.length === 0}
          {t("settings.migration_marker_failed", { reason: marker.reason })}
        {:else}
          <strong>{t("settings.migration_lead")}</strong>
          {#if used.length > 0}
            {t("settings.migration_failed", { items: itemList(used) })}
          {/if}
          {#if unused.length > 0}
            {t("settings.migration_failed_plain", { items: itemList(unused) })}
          {/if}
        {/if}
      </span>
      <button type="button" onclick={() => store.dismissMigrationNote()}>{t("settings.dismiss")}</button>
    </div>
  {/if}
  {#if store.error}
    <div class="banner failure" role="alert">
      <span>
        {store.view
          ? t("settings.save_failed", { error: store.error })
          : t("settings.load_failed", { error: store.error })}
      </span>
      <!-- Before the first load there is nothing behind the error: it can only be tried again, not dismissed. -->
      {#if store.view}
        <button type="button" onclick={() => store.dismissError()}>{t("settings.dismiss")}</button>
      {:else}
        <button type="button" onclick={() => store.load()}>{t("settings.retry")}</button>
      {/if}
    </div>
  {/if}

  {#if store.view}
    <div class="tabs" role="tablist" aria-label={t("tabs.label")}>
      {#each TABS as entry (entry.id)}
        <button
          type="button"
          role="tab"
          id={`tab-${entry.id}`}
          aria-selected={tab === entry.id}
          aria-controls={`panel-${entry.id}`}
          class:selected={tab === entry.id}
          onclick={() => (tab = entry.id)}
        >
          {t(entry.title)}
        </button>
      {/each}
    </div>
    <!-- Every page stays on screen, only hidden, so that what was typed and what is running goes on while another tab is open. -->
    <div class="pages">
      {#each TABS as entry (entry.id)}
        <div role="tabpanel" id={`panel-${entry.id}`} aria-labelledby={`tab-${entry.id}`} hidden={tab !== entry.id}>
          <entry.page {store} />
        </div>
      {/each}
    </div>
  {:else if store.loading}
    <p class="hint loading">{t("settings.loading")}</p>
  {/if}
</div>

<style>
  .settings {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 14px;
    overflow-wrap: anywhere;
  }
  .banner button {
    flex-shrink: 0;
    white-space: nowrap;
  }
  .warning {
    color: var(--warn-fg);
    background: var(--warn-bg);
  }
  .failure {
    color: var(--danger);
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .tabs {
    display: flex;
    gap: 4px;
    justify-content: center;
    padding: 8px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .tabs button {
    padding: 6px 14px;
    color: var(--muted);
    background: transparent;
    border-color: transparent;
  }
  .tabs button.selected {
    color: var(--accent);
    background: var(--accent-soft);
  }
  .pages {
    flex: 1;
    padding: 16px;
    overflow-y: auto;
  }
  .loading {
    padding: 16px;
  }
</style>
