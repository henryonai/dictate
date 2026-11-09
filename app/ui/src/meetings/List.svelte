<script lang="ts">
  import { groupByDay } from "../lib/days";
  import { t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import Row from "./Row.svelte";

  // The recordings in groups by day, with the search box above them.
  let { store }: { store: MeetingsStore } = $props();

  // A window that stays open past midnight changes what "Today" is.
  let now = $state(new Date());
  $effect(() => {
    const clock = setInterval(() => (now = new Date()), 60_000);
    return () => clearInterval(clock);
  });

  const groups = $derived(groupByDay(store.items, now));
</script>

<div class="list">
  <input
    type="search"
    aria-label={t("meetings.search.placeholder")}
    placeholder={t("meetings.search.placeholder")}
    value={store.query}
    oninput={(event) => store.search(event.currentTarget.value)}
  />
  <nav aria-label={t("meetings.list.label")}>
    {#each groups as group (group.label + group.rows[0].id)}
      <section>
        <h2>{group.label}</h2>
        <ul>
          {#each group.rows as row (row.id)}
            <li><Row {row} selected={row.id === store.selectedId} onselect={(id) => store.select(id)} /></li>
          {/each}
        </ul>
      </section>
    {/each}
    {#if groups.length === 0 && store.query.trim() !== ""}
      <p class="hint none">{t("meetings.search.none", { query: store.query.trim() })}</p>
    {/if}
  </nav>
</div>

<style>
  .list {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  input {
    margin: 10px 10px 4px;
    padding: 5px 8px;
    font: inherit;
    color: inherit;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  nav {
    flex: 1;
    min-height: 0;
    padding: 4px 6px 12px;
    overflow-y: auto;
  }
  section {
    margin-bottom: 8px;
  }
  h2 {
    margin: 8px 6px 4px;
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .none {
    margin: 16px 8px;
    overflow-wrap: anywhere;
  }
</style>
