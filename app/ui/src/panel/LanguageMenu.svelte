<script lang="ts">
  import { api } from "../lib/api";
  import { currentLanguage, t } from "../lib/i18n";
  import { languageLabel, languageName } from "./state";

  // The chip with the language of the dictation, and the menu it opens: the favourites, then "Other languages".
  let { language, height = 22 }: { language: string; height?: number } = $props();

  let open = $state(false);
  let othersOpen = $state(false);
  /** The lists, asked of the Rust side when the menu is opened first. */
  let lists = $state<{ favourites: string[]; others: string[] }>();
  let root: HTMLElement | undefined = $state();

  async function toggle() {
    if (open) return close();
    open = true;
    if (!lists) {
      try {
        lists = await api.dictationLanguages();
      } catch {
        // Without the lists the language in use can still be chosen again.
        lists = { favourites: [language], others: [] };
      }
    }
  }

  function close() {
    open = false;
    othersOpen = false;
  }

  async function choose(code: string) {
    close();
    try {
      await api.dictationSetLanguage(code);
    } catch (e) {
      console.warn("Could not change the language:", e);
    }
  }

  // Esc closes the menu and goes no further, so that it doesn't also cancel the dictation; a press anywhere else closes
  // it too. Both listen in the capture phase, ahead of everything that listens on the page.
  $effect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      event.preventDefault();
      close();
    };
    const onPointer = (event: Event) => {
      if (root && event.target instanceof Node && !root.contains(event.target)) close();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("pointerdown", onPointer, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("pointerdown", onPointer, true);
    };
  });

  const ui = $derived(currentLanguage());
</script>

{#snippet item(code: string)}
  <button
    type="button"
    role="menuitemradio"
    aria-checked={code === language}
    data-lang={code}
    class="item"
    class:current={code === language}
    onclick={() => choose(code)}
  >
    <span class="check" aria-hidden="true">{code === language ? "✓" : ""}</span>
    <span class="name">{languageName(code, ui)}</span>
  </button>
{/snippet}

<div class="lang" bind:this={root}>
  <button
    type="button"
    class="chip"
    style:height="{height}px"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={t("panel.language_tooltip")}
    title={t("panel.language_tooltip")}
    onclick={toggle}>{languageLabel(language)}</button
  >
  {#if open && lists}
    <!-- Counted in the size of the window, which grows to hold the menu. -->
    <div class="menu" role="menu" data-extent>
      {#each lists.favourites as code (code)}
        {@render item(code)}
      {/each}
      {#if lists.others.length > 0}
        <div class="rule" role="separator"></div>
        <button
          type="button"
          role="menuitem"
          aria-haspopup="menu"
          aria-expanded={othersOpen}
          class="item more"
          onclick={() => (othersOpen = !othersOpen)}
        >
          <span class="check" aria-hidden="true"></span>
          {t("panel.other_languages")}
          <span class="arrow" aria-hidden="true">{othersOpen ? "▾" : "▸"}</span>
        </button>
        {#if othersOpen}
          <div role="menu" aria-label={t("panel.other_languages")}>
            {#each lists.others as code (code)}
              {@render item(code)}
            {/each}
          </div>
        {/if}
      {/if}
    </div>
  {/if}
</div>

<style>
  .lang {
    position: relative;
    flex: none;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    padding: 0 10px;
    border: 0;
    border-radius: 999px;
    background: rgb(59 59 59);
    color: rgb(245 245 245);
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
  }

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 10;
    box-sizing: border-box;
    width: 210px;
    max-height: 260px;
    overflow-y: auto;
    padding: 4px;
    border: 1px solid rgb(255 255 255 / 0.1);
    border-radius: 10px;
    background: rgb(40 40 40);
    box-shadow: 0 6px 16px rgb(0 0 0 / 0.4);
    color: rgb(245 245 245);
    font-size: 12px;
    font-weight: 400;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 4px;
    width: 100%;
    padding: 4px 8px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .item:hover,
  .item:focus-visible {
    background: rgb(255 255 255 / 0.12);
    outline: none;
  }

  .check {
    display: inline-block;
    width: 14px;
    flex: none;
  }

  .arrow {
    margin-left: auto;
    color: rgb(255 255 255 / 0.6);
  }

  .rule {
    height: 1px;
    margin: 4px 0;
    background: rgb(255 255 255 / 0.1);
  }
</style>
