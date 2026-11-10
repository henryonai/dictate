<script lang="ts">
  import type { ErrorView } from "../lib/api";
  import { Flash } from "../lib/flash.svelte";
  import { errorMessage } from "../lib/meeting-errors";
  import { t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";

  // Why a recording is not done, and what to do about it. The buttons follow the kind of the error: the Rust side names it.
  let {
    store,
    id,
    error,
    hasTranscript,
  }: { store: MeetingsStore; id: string; error: ErrorView; hasTranscript: boolean } = $props();

  const needsTool = $derived(error.kind === "MissingTool" || error.kind === "Unsupported");
  const title = $derived.by(() => {
    if (error.kind === "Api") return t("meetings.errorcard.api");
    if (needsTool) return t("meetings.errorcard.tool");
    if (error.kind === "Cancelled") return t("meetings.error.cancelled");
    return t("meetings.errorcard.generic");
  });
  const message = $derived(errorMessage(error));

  const copied = new Flash();
  $effect(() => () => copied.clear());

  async function copyCommand() {
    if (error.command && (await store.copy(error.command))) copied.show();
  }
</script>

<section class="card" aria-labelledby="error-card-title">
  <h2 id="error-card-title">{title}</h2>
  <!-- The title of a cancelled job is the whole message. -->
  {#if message !== title}<p class="message">{message}</p>{/if}
  {#if hasTranscript && error.kind !== "Cancelled"}<p class="hint">{t("meetings.errorcard.kept")}</p>{/if}
  <div class="actions">
    {#if error.kind === "Api"}
      <button type="button" onclick={() => store.openSettings("accounts")}>{t("meetings.errorcard.open_api")}</button>
      <button type="button" class="primary" onclick={() => store.resummarize(id)}>{t("meetings.errorcard.retry_summary")}</button>
    {:else}
      {#if needsTool && error.command}
        <button type="button" onclick={copyCommand}>{copied.on ? t("meetings.copied") : t("meetings.errorcard.copy_command")}</button>
      {:else if error.kind === "NoModel"}
        <button type="button" onclick={() => store.openSettings("speech")}>{t("meetings.open_settings")}</button>
      {/if}
      <button type="button" class="primary" onclick={() => store.retry(id)}>{t("meetings.errorcard.retry")}</button>
    {/if}
    <button type="button" class="link" onclick={() => store.showLog(id)}>{t("meetings.errorcard.view_log")}</button>
  </div>
</section>

<style>
  .card {
    margin: 0 0 16px;
    padding: 12px 14px;
    background: var(--surface);
    border: 1px solid var(--danger);
    border-radius: 10px;
  }
  h2 {
    margin: 0 0 6px;
    font-size: 14px;
    letter-spacing: normal;
    text-transform: none;
    color: var(--danger);
  }
  .message {
    margin: 0 0 6px;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .hint {
    margin: 0 0 6px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-top: 10px;
  }
  .link {
    margin-left: auto;
    padding-inline: 4px;
    color: var(--accent);
    background: transparent;
    border-color: transparent;
    text-decoration: underline;
  }
</style>
