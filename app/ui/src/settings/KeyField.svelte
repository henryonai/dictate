<script lang="ts">
  import { t } from "../lib/i18n";
  import Field from "./Field.svelte";

  // An API key is only ever typed here and sent away: the field starts empty, and what is saved shows as "Saved".
  let {
    label,
    saved,
    draft = $bindable(""),
    onsave,
    onremove,
  }: {
    label: string;
    saved: boolean;
    draft: string;
    onsave: () => void;
    onremove: () => void;
  } = $props();
</script>

<form
  onsubmit={(event) => {
    event.preventDefault();
    if (draft.trim()) onsave();
  }}
>
  <Field {label}>
    <input
      type="password"
      bind:value={draft}
      autocomplete="off"
      spellcheck="false"
      placeholder={saved ? t("accounts.key_placeholder_replace") : t("accounts.key_placeholder")}
    />
  </Field>
  <div class="key-row">
    {#if saved}
      <span class="badge ok">{t("accounts.key_saved")}</span>
    {:else}
      <span class="hint">{t("accounts.key_missing")}</span>
    {/if}
    <button type="submit" disabled={!draft.trim()}>{t("accounts.save_key")}</button>
    {#if saved}
      <button type="button" onclick={onremove}>{t("accounts.remove_key")}</button>
    {/if}
  </div>
</form>

<style>
  .key-row {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding-bottom: 4px;
  }
  .key-row .hint {
    margin: 0;
  }
</style>
