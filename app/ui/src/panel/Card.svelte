<script lang="ts">
  import { untrack } from "svelte";
  import { api } from "../lib/api";
  import { currentLanguage, t } from "../lib/i18n";
  import { CARD_MORE_STYLES, CARD_STYLES, cardHeader, cardMeta, cardShortcut, errorText, noticeText, onMac, type PanelState } from "./state";

  // The review card: the text in a box to edit, the styles to write it in again, and what to do with it. It comes up
  // when there is no field to write into, and when "Edit…" of the chip is pressed. It takes the keyboard.
  let { panel }: { panel: PanelState } = $props();

  const mac = onMac();
  /** How the shortcuts are written: ⌘1 on a Mac, Ctrl+1 elsewhere. */
  const mod = mac ? "⌘" : "Ctrl+";
  const undoKeys = mac ? "⌥⌘Z" : "Ctrl+Alt+Z";
  const ui = $derived(currentLanguage());

  // The box is the person's to edit: what the Rust side sends replaces it only when it is new text (the answer of the AI,
  // the text before the edit), and a state that carries the same text again leaves what was typed alone.
  const refining = $derived(panel.phase === "refining");

  // The answer of the AI always is the box's text, though it is the same as the text that was sent before (a rewrite
  // that comes out as an earlier draft): what was typed meanwhile is not the answer.
  let text = $state(untrack(() => panel.text));
  let sent = untrack(() => panel.text);
  let wasRefining = untrack(() => panel.phase === "refining");
  $effect(() => {
    const next = panel.text;
    const answered = wasRefining && !refining;
    wasRefining = refining;
    if (answered || next !== sent) {
      sent = next;
      text = next;
    }
  });

  let customOpen = $state(untrack(() => panel.openCustom));
  $effect(() => {
    if (panel.openCustom) customOpen = true;
  });
  let custom = $state("");
  let moreOpen = $state(false);

  let box: HTMLTextAreaElement | undefined = $state();
  let customField: HTMLInputElement | undefined = $state();
  let moreRoot: HTMLElement | undefined = $state();

  const empty = $derived(text.trim() === "");
  /** The styles, the custom request and the copy wait for the AI, and for something to work on. */
  const busy = $derived(refining || empty);
  const header = $derived(cardHeader(panel));
  const hint = $derived(
    [
      t("card.hint_edit", { mod }),
      panel.original !== null ? t("card.hint_undo", { undo: undoKeys }) : null,
      t("card.hint_discard"),
    ]
      .filter((part) => part !== null)
      .join(" · "),
  );

  // The keyboard goes to the request when it is open, and to the box otherwise, and goes back when the AI is done.
  $effect(() => {
    if (refining) return;
    if (customOpen && customField) {
      customField.focus();
    } else if (box) {
      box.focus();
      box.setSelectionRange(box.value.length, box.value.length);
    }
  });

  // The box is as tall as its text, between 90 and 260 pixels.
  $effect(() => {
    void text;
    if (!box) return;
    box.style.height = "auto";
    box.style.height = `${Math.min(260, Math.max(90, box.scrollHeight))}px`;
  });

  // A press anywhere else closes the menu of the other styles.
  $effect(() => {
    if (!moreOpen) return;
    const onPointer = (event: Event) => {
      if (moreRoot && event.target instanceof Node && !moreRoot.contains(event.target)) moreOpen = false;
    };
    window.addEventListener("pointerdown", onPointer, true);
    return () => window.removeEventListener("pointerdown", onPointer, true);
  });

  /** Calls the Rust side and says if it fails: nothing in the card waits for the answer. */
  function ask(what: string, call: Promise<void>) {
    call.catch((e) => console.warn(`Could not ${what}:`, e));
  }

  /** What works on the text of the box is told that text first: the Rust side has no other way to know what was typed. */
  async function work(what: string, call: () => Promise<void>) {
    try {
      await api.dictationCardText(text);
      await call();
    } catch (e) {
      console.warn(`Could not ${what}:`, e);
    }
  }

  function rewrite(style: string) {
    if (busy) return;
    moreOpen = false;
    void work("rewrite the text", () => api.dictationRewrite(style));
  }

  function runCustom() {
    const instruction = custom.trim();
    if (busy || instruction === "") return;
    void work("edit the text", () => api.dictationCustom(instruction));
  }

  function undo() {
    if (panel.original === null || refining) return;
    ask("undo the edit", api.dictationUndo());
  }

  function copy() {
    if (busy) return;
    ask("copy the text", api.dictationCopy(text));
  }

  function pasteIntoField() {
    if (busy || !panel.canWrite) return;
    ask("paste the text", api.dictationPasteCard(text));
  }

  function discard() {
    ask("drop the dictation", api.dictationCancel());
  }

  /** Cmd (Ctrl) and Return: the text goes to the field if it came from there, else to the clipboard. */
  function confirm() {
    if (panel.fromChip) pasteIntoField();
    else copy();
  }

  function onkeydown(event: KeyboardEvent) {
    // An input method that composes a word takes Esc and Return for itself.
    if (event.isComposing) return;
    const shortcut = cardShortcut(event, mac);
    if (!shortcut) return;
    event.preventDefault();
    switch (shortcut.kind) {
      case "discard":
        // Esc closes a menu that is open, and the next one drops the dictation.
        if (moreOpen) moreOpen = false;
        else discard();
        break;
      case "style":
        rewrite(shortcut.style);
        break;
      case "undo":
        undo();
        break;
      case "confirm":
        confirm();
        break;
    }
  }

  function onCustomKeydown(event: KeyboardEvent) {
    if (event.key !== "Enter" || event.isComposing || event.metaKey || event.ctrlKey || event.altKey) return;
    event.preventDefault();
    runCustom();
  }
</script>

<svelte:window {onkeydown} />

{#snippet style(id: string, index: number)}
  <button
    type="button"
    class="style"
    class:current={panel.style === id}
    aria-pressed={panel.style === id}
    aria-keyshortcuts={`${mac ? "Meta" : "Control"}+${index + 1}`}
    disabled={busy}
    onclick={() => rewrite(id)}
  >
    {t(`presets.${id}`)}<kbd aria-hidden="true">{mod}{index + 1}</kbd>
  </button>
{/snippet}

<div class="card">
  <div class="header">
    <span class="dot" class:refining></span>
    <span class="title">{header}</span>
    <span class="grow"></span>
    <span class="meta">{cardMeta(panel, ui)}</span>
  </div>

  <div class="well">
    <textarea
      bind:this={box}
      bind:value={text}
      disabled={refining}
      aria-label={t("card.text_label")}
      spellcheck="false"
    ></textarea>
    {#if refining}
      <span class="spinner" role="status" aria-label={header}></span>
    {/if}
  </div>

  {#if panel.error}
    <div class="error" role="alert">
      <span>{errorText(panel.error)}</span>
      {#if panel.error.action === "open_settings"}
        <button type="button" class="link" onclick={() => ask("open Settings", api.dictationOpenSettings())}>
          {t("dictation.open_settings")}
        </button>
      {/if}
    </div>
  {/if}

  {#if panel.notice}
    <p class="error" role="alert">{noticeText(panel.notice)}</p>
  {/if}

  {#if panel.accessibilityMissing}
    <div class="access">
      <span>{t("card.accessibility_hint")}</span>
      <button type="button" class="link" onclick={() => ask("ask for the permission", api.dictationGrantAccessibility())}>
        {t("card.grant_access")}
      </button>
    </div>
  {/if}

  <div class="styles">
    {#each CARD_STYLES as id, index (id)}
      {@render style(id, index)}
    {/each}
    <div class="more" bind:this={moreRoot}>
      <button type="button" class="style" aria-haspopup="menu" aria-expanded={moreOpen} disabled={busy} onclick={() => (moreOpen = !moreOpen)}>
        {t("card.more")}<span class="caret" aria-hidden="true">▾</span>
      </button>
      {#if moreOpen}
        <!-- Counted in the size of the window, which grows to hold the menu. -->
        <div class="menu" role="menu" data-extent>
          {#each CARD_MORE_STYLES as id (id)}
            <button type="button" role="menuitemradio" aria-checked={panel.style === id} class="item" onclick={() => rewrite(id)}>
              <span class="check" aria-hidden="true">{panel.style === id ? "✓" : ""}</span>
              {t(`presets.${id}`)}
            </button>
          {/each}
        </div>
      {/if}
    </div>
    <button type="button" class="style" aria-expanded={customOpen} onclick={() => (customOpen = !customOpen)}>
      {t("card.custom_toggle")}
    </button>
  </div>

  {#if customOpen}
    <div class="custom">
      <input
        bind:this={customField}
        bind:value={custom}
        type="text"
        placeholder={t("card.custom_placeholder")}
        aria-label={t("card.custom_placeholder")}
        disabled={refining}
        onkeydown={onCustomKeydown}
      />
      <button type="button" class="primary" disabled={busy || custom.trim() === ""} onclick={runCustom}>{t("card.custom_run")}</button>
    </div>
  {/if}

  <div class="footer">
    {#if panel.original !== null}
      <button type="button" class="link" disabled={refining} onclick={undo}>{t("card.undo")}</button>
    {/if}
    <span class="hint">{hint}</span>
    <span class="grow"></span>
    <button type="button" class="secondary" onclick={discard}>{t("card.discard")}</button>
    {#if panel.fromChip}
      <button type="button" class="secondary" disabled={busy} onclick={copy}>{t("card.copy")}</button>
      <button type="button" class="primary" disabled={busy || !panel.canWrite} onclick={pasteIntoField}>{t("card.paste_into_field")}</button>
    {:else}
      <button type="button" class="primary" disabled={busy} onclick={copy}>{t("card.copy_close")}</button>
    {/if}
  </div>
</div>

<style>
  .card {
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: 540px;
    padding: 16px;
    border: 1px solid rgb(255 255 255 / 0.1);
    border-radius: 16px;
    background: rgb(37 37 37);
    box-shadow: 0 10px 28px rgb(0 0 0 / 0.4);
    color: rgb(245 245 245);
  }

  .header {
    display: flex;
    align-items: center;
    gap: 8px;
    color: rgb(184 184 184);
    font-size: 12px;
    font-weight: 500;
  }

  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: rgb(48 209 88);
  }

  .dot.refining {
    background: rgb(191 90 242);
  }

  .grow {
    flex: 1;
  }

  .meta {
    font-variant-numeric: tabular-nums;
  }

  .well {
    position: relative;
  }

  textarea {
    box-sizing: border-box;
    display: block;
    width: 100%;
    min-height: 90px;
    max-height: 260px;
    padding: 10px 12px;
    border: 1px solid rgb(255 255 255 / 0.1);
    border-radius: 10px;
    background: rgb(28 28 28);
    color: inherit;
    font: inherit;
    font-size: 15px;
    line-height: 1.4;
    resize: none;
    /* The text is the one thing of the card that is typed in and selected. */
    user-select: text;
    -webkit-user-select: text;
    cursor: text;
  }

  textarea:disabled {
    opacity: 0.6;
  }

  .spinner {
    position: absolute;
    top: 10px;
    right: 10px;
    box-sizing: border-box;
    width: 14px;
    height: 14px;
    border: 1.5px solid rgb(255 255 255 / 0.25);
    border-top-color: rgb(255 255 255 / 0.8);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .error {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 10px;
    margin: 0;
    color: rgb(255 159 10);
    font-size: 12px;
  }

  .access {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 8px;
    background: rgb(255 159 10 / 0.14);
    font-size: 12px;
  }

  .styles {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .style {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 10px;
    border: 1px solid rgb(255 255 255 / 0.1);
    border-radius: 13px;
    background: rgb(255 255 255 / 0.06);
    color: inherit;
    font: inherit;
    font-size: 12px;
    white-space: nowrap;
    cursor: pointer;
  }

  .style:hover:not(:disabled) {
    background: rgb(255 255 255 / 0.12);
  }

  .style.current {
    border-color: rgb(76 154 255);
    background: rgb(76 154 255 / 0.25);
  }

  .style:disabled {
    opacity: 0.45;
    cursor: default;
  }

  kbd {
    color: rgb(255 255 255 / 0.5);
    font: inherit;
    font-size: 10px;
  }

  .caret {
    color: rgb(255 255 255 / 0.6);
    font-size: 10px;
  }

  .more {
    position: relative;
  }

  .menu {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 10;
    box-sizing: border-box;
    min-width: 210px;
    padding: 4px;
    border: 1px solid rgb(255 255 255 / 0.1);
    border-radius: 10px;
    background: rgb(40 40 40);
    box-shadow: 0 6px 16px rgb(0 0 0 / 0.4);
    font-size: 12px;
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
    white-space: nowrap;
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

  .custom {
    display: flex;
    gap: 8px;
  }

  input {
    flex: 1;
    min-width: 0;
    height: 30px;
    padding: 0 10px;
    border: 1px solid rgb(255 255 255 / 0.1);
    border-radius: 8px;
    background: rgb(28 28 28);
    color: inherit;
    font: inherit;
    user-select: text;
    -webkit-user-select: text;
    cursor: text;
  }

  .footer {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .hint {
    color: rgb(140 140 140);
    font-size: 11px;
  }

  .primary,
  .secondary {
    height: 30px;
    padding: 0 14px;
    border: 0;
    border-radius: 8px;
    font: inherit;
    font-weight: 600;
    white-space: nowrap;
    cursor: pointer;
  }

  .primary {
    background: rgb(245 245 245);
    color: black;
  }

  .secondary {
    background: rgb(255 255 255 / 0.1);
    color: inherit;
  }

  .primary:disabled,
  .secondary:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .link {
    padding: 0;
    border: 0;
    background: transparent;
    color: rgb(76 154 255);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }

  .link:disabled {
    opacity: 0.45;
    cursor: default;
  }
</style>
