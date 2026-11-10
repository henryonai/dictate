<script lang="ts">
  import { t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import { onMac } from "../panel/state";
  import LinkForm from "./LinkForm.svelte";

  // What the window shows when the library has nothing yet: where to start, and whether what is needed is ready. The cards
  // for recording the screen and the rows for its permissions are not here: recording is not part of this window yet.
  let { store }: { store: MeetingsStore } = $props();

  /** How the paste is written: ⌘V on a Mac, Ctrl+V elsewhere. */
  const mod = onMac() ? "⌘" : "Ctrl+";
</script>

{#snippet ready(name: string, working: boolean | undefined, tab: "accounts" | "speech", action: string)}
  <li class="ready-row">
    <span>{name}</span>
    {#if working === undefined}
      <span class="hint">{t("meetings.empty.checking")}</span>
    {:else if working}
      <span class="ok">{t("meetings.empty.working")}</span>
    {:else}
      <button type="button" onclick={() => store.openSettings(tab)}>{action}</button>
    {/if}
  </li>
{/snippet}

<div class="empty">
  <h1>{t("meetings.empty.heading")}</h1>
  <p class="sub">{t("meetings.empty.sub")}</p>

  <div class="cards">
    <button type="button" class="card" onclick={() => store.pickFiles()}>
      <strong>{t("meetings.empty.file_title")}</strong>
      <span class="hint">{t("meetings.empty.file_sub")}</span>
    </button>
    <section class="card" aria-labelledby="empty-link-title">
      <h2 id="empty-link-title">{t("meetings.empty.link_title")}</h2>
      <LinkForm {store} />
      <span class="hint">{t("meetings.empty.link_sub", { mod })}</span>
    </section>
  </div>

  <div class="drop">{t("meetings.empty.drop")}</div>

  <section class="group" aria-labelledby="ready-title">
    <h2 id="ready-title">{t("meetings.empty.ready")}</h2>
    <ul>
      {@render ready(t("meetings.empty.api"), store.readiness?.ai, "accounts", t("meetings.open_settings"))}
      {@render ready(t("meetings.empty.whisper"), store.readiness?.whisper, "speech", t("meetings.empty.download_model"))}
    </ul>
  </section>
</div>

<style>
  .empty {
    max-width: 640px;
    margin: 0 auto;
    padding: 32px 16px;
  }
  h1 {
    margin: 0 0 4px;
    font-size: 20px;
  }
  .sub {
    margin: 0 0 20px;
    color: var(--muted);
  }
  .cards {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    margin-bottom: 12px;
  }
  .card {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    padding: 12px 14px;
    text-align: left;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
  }
  button.card:hover {
    border-color: var(--accent);
  }
  .card h2 {
    margin: 0;
    font-size: 13px;
    letter-spacing: normal;
    text-transform: none;
    color: var(--fg);
  }
  section.card :global(form) {
    width: 100%;
  }
  .card .hint {
    margin: 0;
  }
  .drop {
    margin-bottom: 20px;
    padding: 18px;
    text-align: center;
    color: var(--muted);
    border: 2px dashed var(--border);
    border-radius: 10px;
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .ready-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 0;
  }
  .ok {
    color: var(--ok);
  }
  .ok::before {
    content: "✓ ";
  }
</style>
