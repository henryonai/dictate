<script lang="ts">
  import { tick } from "svelte";
  import { formatClock } from "../lib/days";
  import { t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import { recordHotkey } from "../lib/recording";
  import LinkForm from "./LinkForm.svelte";
  import Modal from "./Modal.svelte";

  // The toolbar: the list's toggle, the red button that records (it is "Stop" with the time while a recording goes on), the button
  // that opens the choice of a file or a link, and Settings.
  let { store }: { store: MeetingsStore } = $props();

  let addOpen = $state(false);
  let addRoot: HTMLElement | undefined = $state();
  let addButton: HTMLButtonElement | undefined = $state();

  /** The toggle is for a window that has recordings (or a search going on). */
  const hasRecordings = $derived(store.items.length > 0 || store.query.trim() !== "");

  /** A recording is made, paused or put together: the button stops it. */
  const stopping = $derived(store.isRecording && store.recording.phase !== "starting");
  /** What the button asked for is under way: the recording is being started, or being put together. */
  const waiting = $derived(store.recording.phase === "starting" || store.recording.phase === "finishing");
  const hotkey = recordHotkey();

  function record() {
    if (stopping) void store.stopRecording();
    else store.openRecorder();
  }

  async function chooseFile() {
    addOpen = false;
    await store.pickFiles();
  }

  function openLink() {
    addOpen = false;
    store.openLinkDialog();
  }

  async function closeLink() {
    store.closeLinkDialog();
    await tick();
    addButton?.focus();
  }

  function onkeydown(event: KeyboardEvent) {
    if (addOpen && !store.modalOpen && event.key === "Escape") {
      event.preventDefault();
      addOpen = false;
      addButton?.focus();
    }
  }

  function onpointerdown(event: PointerEvent) {
    if (addOpen && addRoot && event.target instanceof Node && !addRoot.contains(event.target)) addOpen = false;
  }
</script>

<svelte:window {onkeydown} {onpointerdown} />

<header class="toolbar" role="toolbar" aria-label={t("meetings.toolbar.label")}>
  {#if hasRecordings}
    <button type="button" aria-expanded={store.sidebarOpen} aria-controls="library-list" onclick={() => store.toggleSidebar()}>
      {store.sidebarOpen ? t("meetings.list.hide") : t("meetings.list.show")}
    </button>
  {/if}
  <span class="spacer"></span>
  <button
    type="button"
    class="record"
    disabled={waiting}
    title={stopping ? t("recording.hud.stop_tip", { hotkey }) : t("meetings.record.tip", { hotkey })}
    onclick={record}
  >
    <span class="mark" class:square={stopping} aria-hidden="true"></span>
    {stopping ? t("meetings.record.stop", { time: formatClock(store.recording.elapsedMs / 1000) }) : t("meetings.record")}
  </button>
  <!-- A button that shows two choices: a disclosure, with plain buttons. -->
  <div class="add" bind:this={addRoot}>
    <button
      type="button"
      aria-expanded={addOpen}
      aria-controls={addOpen ? "add-choices" : undefined}
      bind:this={addButton}
      onclick={() => (addOpen = !addOpen)}
    >
      <span aria-hidden="true">+</span> {t("meetings.add")}
    </button>
    {#if addOpen}
      <div class="choices" id="add-choices" role="group" aria-label={t("meetings.add")}>
        <button type="button" onclick={chooseFile}>{t("meetings.add.file")}</button>
        <button type="button" onclick={openLink}>{t("meetings.add.link")}</button>
      </div>
    {/if}
  </div>
  <button type="button" onclick={() => store.openSettings("general")}>{t("meetings.settings")}</button>
</header>

{#if store.linkDialogOpen}
  <Modal labelledby="link-dialog-title" onclose={closeLink}>
    <div class="link-dialog">
      <h2 id="link-dialog-title">{t("meetings.link.title")}</h2>
      <p class="hint">{t("meetings.link.hint")}</p>
      <LinkForm {store} prefill autofocus oncancel={closeLink} ondone={closeLink} />
    </div>
  </Modal>
{/if}

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .spacer {
    flex: 1;
  }
  .record {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: #fff;
    background: #d92d20;
    border-color: #d92d20;
    font-variant-numeric: tabular-nums;
  }
  .mark {
    width: 9px;
    height: 9px;
    background: #fff;
    border-radius: 50%;
  }
  .mark.square {
    border-radius: 2px;
  }
  .add {
    position: relative;
  }
  .choices {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    z-index: 10;
    display: flex;
    flex-direction: column;
    min-width: 260px;
    padding: 4px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 6px 16px rgb(0 0 0 / 0.25);
  }
  .choices button {
    padding: 6px 10px;
    text-align: left;
    background: transparent;
    border-color: transparent;
  }
  .choices button:hover,
  .choices button:focus-visible {
    background: var(--accent-soft);
  }
  .link-dialog {
    width: min(460px, calc(100vw - 64px));
  }
  .link-dialog h2 {
    margin: 0 0 4px;
    font-size: 14px;
    letter-spacing: normal;
    text-transform: none;
    color: var(--fg);
  }
</style>
