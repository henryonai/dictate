<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";

  // A field for a link, and what sends it to be summarized. The empty state has one on its card, and the dialog of "Paste a
  // link" has one. A link that is refused is said next to the field.
  let {
    store,
    prefill = false,
    autofocus = false,
    oncancel,
    ondone,
  }: {
    store: MeetingsStore;
    /** Fill the field with the clipboard, when what is copied is a link. */
    prefill?: boolean;
    autofocus?: boolean;
    /** The dialog has a Cancel button; the card has none. */
    oncancel?: () => void;
    /** The link was imported. */
    ondone?: () => void;
  } = $props();

  let value = $state("");
  let failure = $state("");
  let working = $state(false);
  let input: HTMLInputElement | undefined = $state();

  onMount(() => {
    if (autofocus) input?.focus();
    if (prefill) void fillFromClipboard();
  });

  // The page has no command to read the clipboard (it may only copy), so it asks the web view, which may refuse: then the
  // field stays empty and the person pastes into it.
  async function fillFromClipboard() {
    try {
      const copied = (await navigator.clipboard.readText()).trim();
      if (/^https?:\/\//i.test(copied) && value === "") value = copied;
    } catch {
      // Nothing was copied that the page could read.
    }
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const url = value.trim();
    if (!url || working) return;
    working = true;
    failure = "";
    const problem = await store.importLink(url);
    working = false;
    if (problem) {
      failure = problem;
    } else {
      value = "";
      ondone?.();
    }
  }
</script>

<form onsubmit={submit}>
  <div class="row">
    <input
      type="text"
      inputmode="url"
      autocomplete="off"
      spellcheck="false"
      aria-label={t("meetings.link.label")}
      placeholder={t("meetings.link.placeholder")}
      bind:value
      bind:this={input}
      oninput={() => (failure = "")}
    />
    <button type="submit" class="primary" disabled={working || value.trim() === ""}>{t("meetings.link.submit")}</button>
    {#if oncancel}<button type="button" onclick={oncancel}>{t("common.cancel")}</button>{/if}
  </div>
  {#if failure}<p class="error" role="alert">{failure}</p>{/if}
</form>

<style>
  .row {
    display: flex;
    gap: 8px;
  }
  input {
    flex: 1;
    min-width: 0;
  }
</style>
