<script lang="ts">
  import { api } from "../lib/api";
  import { t } from "../lib/i18n";
  import { chipMenuStyles, type PanelState } from "./state";

  // The menu of the ⋯ button of the chip: write the text again in another style, or open it in the card, or copy it.
  // "Original (unedited)" and "Delete what was written" are not here: the first release writes no text in place, so a
  // rewrite is always a fresh version pasted at the caret.
  let { panel, onclose }: { panel: PanelState; onclose: () => void } = $props();

  const styles = $derived(chipMenuStyles(panel.style));

  /** Runs what was chosen, and closes the menu: whatever the choice does, the menu has done its work. */
  function choose(action: () => Promise<void>, what: string) {
    onclose();
    action().catch((e) => console.warn(`Could not ${what}:`, e));
  }
</script>

<!-- Counted in the size of the window, which grows to hold the menu. -->
<div class="menu" role="menu" data-extent>
  <div class="header" role="menuitem" aria-disabled="true">{t("chip.rewrite_as")}</div>
  {#each styles as style (style)}
    <button type="button" role="menuitem" class="item" onclick={() => choose(() => api.dictationRewrite(style), "rewrite the text")}>
      {t(`presets.${style}`)}
    </button>
  {/each}
  <div class="rule" role="separator"></div>
  <button type="button" role="menuitem" class="item" onclick={() => choose(() => api.dictationOpenCard(true), "open the card")}>
    {t("chip.custom")}
  </button>
  <button type="button" role="menuitem" class="item" onclick={() => choose(() => api.dictationOpenCard(false), "open the card")}>
    {t("chip.edit")}
  </button>
  <button type="button" role="menuitem" class="item" onclick={() => choose(() => api.dictationCopy(panel.text), "copy the text")}>
    {t("chip.copy")}
  </button>
</div>

<style>
  .menu {
    box-sizing: border-box;
    min-width: 230px;
    padding: 4px;
    border: 1px solid rgb(255 255 255 / 0.1);
    border-radius: 10px;
    background: rgb(40 40 40);
    box-shadow: 0 6px 16px rgb(0 0 0 / 0.4);
    color: rgb(245 245 245);
    font-size: 12px;
    font-weight: 400;
  }

  .header {
    padding: 4px 8px;
    color: rgb(255 255 255 / 0.5);
    font-size: 11px;
  }

  .item {
    display: block;
    width: 100%;
    padding: 4px 8px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    white-space: nowrap;
    cursor: pointer;
  }

  .item:hover,
  .item:focus-visible {
    background: rgb(255 255 255 / 0.12);
    outline: none;
  }

  .rule {
    height: 1px;
    margin: 4px 0;
    background: rgb(255 255 255 / 0.1);
  }
</style>
