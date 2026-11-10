<script lang="ts">
  import { onMount } from "svelte";
  import type { MeetingDetail } from "../lib/api";
  import { t } from "../lib/i18n";
  import { errorMessage } from "../lib/meeting-errors";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import type { PlayerController } from "../lib/player.svelte";
  import { MAX_NAME_LENGTH, planRenames } from "../lib/speaker-names";
  import Toggle from "../settings/Toggle.svelte";
  import Modal from "./Modal.svelte";

  // The sheet that gives the speakers of a recording their real names: in the transcript and the summary, without AI (the summary can
  // be written again from the new names, which does call it). It is made when the rename is asked for, so what was typed in an earlier
  // one is not in it. It is open over `detail`, the recording that is shown.
  let { store, detail, player }: { store: MeetingsStore; detail: MeetingDetail; player: PlayerController } = $props();

  /** The speakers as they first talk. */
  const speakers = $derived(detail.speakers);
  /** What was typed in the field of each speaker, by the name that the speaker has now. */
  let typed = $state<Record<string, string>>({});
  let resummarize = $state(false);
  let saving = $state(false);
  let failure = $state("");
  /** The sheet played a sample of the recording: the player is stopped when it closes. */
  let listened = false;
  let gone = false;
  let sheet: HTMLElement | undefined = $state();

  const plan = $derived(planRenames(speakers.map((speaker) => speaker.name), typed));
  const canRename = $derived(Object.keys(plan.names).length > 0);
  /** A summary to write again, and a model to write it. */
  const canResummarize = $derived(detail.summary !== null && store.readiness?.ai === true);

  function listen(start: number) {
    listened = true;
    void player.seek(start);
  }

  /**
   * Closes the sheet; a sample that it played does not go on. Not while the names are being saved: a save that fails says so in the
   * sheet, and one that is closed meanwhile would lose that (and what was typed).
   */
  function close() {
    if (saving) return;
    if (listened) player.pause();
    store.closeRename();
  }

  async function submit() {
    if (!canRename || saving) return;
    saving = true;
    failure = "";
    const problem = await store.renameSpeakers(detail.row.id, plan.names, resummarize && canResummarize);
    if (gone) return;
    saving = false;
    if (problem) {
      failure = t("meetings.speakers.save_failed", { reason: errorMessage(problem) });
      return;
    }
    close();
  }

  // The sheet takes the focus while it is open, on the field of the speaker whose name was clicked (the first one when it was asked
  // for as a whole), and gives it back to what had it.
  onMount(() => {
    const before = document.activeElement;
    const fields = sheet?.querySelectorAll<HTMLInputElement>("input[type='text']") ?? [];
    const named = speakers.findIndex((speaker) => speaker.name === store.renameFor?.speaker);
    fields[Math.max(0, named)]?.focus();
    return () => {
      gone = true;
      if (before instanceof HTMLElement && before.isConnected) before.focus();
    };
  });
</script>

<Modal labelledby="speakers-title" onclose={close}>
  <form
    class="sheet"
    bind:this={sheet}
    onsubmit={(event) => {
      event.preventDefault();
      void submit();
    }}
  >
    <h2 id="speakers-title">{t("meetings.speakers.title")}</h2>
    <p class="hint">{t("meetings.speakers.explainer")}</p>

    <ul class="speakers" class:scrolls={speakers.length > 5}>
      {#each speakers as speaker (speaker.name)}
        {@const label = t("meetings.speakers.listen", { name: speaker.name, stamp: speaker.sample.stamp })}
        <li>
          <div class="about">
            <p class="who">
              <strong>{speaker.name}</strong>
              <span class="count">{speaker.lines === 1 ? t("meetings.speakers.line") : t("meetings.speakers.lines", { n: speaker.lines })}</span>
            </p>
            <p class="sample">“{speaker.sample.text}”</p>
          </div>
          <button
            type="button"
            class="listen"
            aria-label={label}
            title={player.available ? label : t("meetings.speakers.no_media")}
            disabled={!player.available}
            onclick={() => listen(speaker.sample.start)}
          >
            ▶
          </button>
          <input
            type="text"
            value={typed[speaker.name] ?? ""}
            placeholder={t("meetings.speakers.new_name")}
            aria-label={t("meetings.speakers.new_name_for", { name: speaker.name })}
            oninput={(event) => (typed[speaker.name] = event.currentTarget.value)}
          />
        </li>
      {/each}
    </ul>

    {#each plan.merges as merge (merge.into)}
      <p class="warning" role="status">{t("meetings.speakers.merge", { names: merge.from.join(" + "), name: merge.into })}</p>
    {/each}
    {#if plan.trimmed}
      <p class="hint">{t("meetings.speakers.too_long", { max: MAX_NAME_LENGTH })}</p>
    {/if}
    {#if canResummarize}
      <Toggle label={t("meetings.speakers.resummarize")} checked={resummarize} onchange={(checked) => (resummarize = checked)} />
    {/if}
    {#if failure}
      <p class="error" role="alert">{failure}</p>
    {/if}

    <div class="actions">
      <button type="button" disabled={saving} onclick={close}>{t("common.cancel")}</button>
      <button type="submit" class="primary" disabled={!canRename || saving}>{t("meetings.speakers.rename")}</button>
    </div>
  </form>
</Modal>

<style>
  .sheet {
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: min(580px, calc(100vw - 64px));
    min-height: 0;
  }
  h2 {
    margin: 0;
    font-size: 14px;
    letter-spacing: normal;
    text-transform: none;
    color: var(--fg);
  }
  .hint {
    margin: 0;
  }
  .speakers {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .speakers.scrolls {
    max-height: 380px;
    overflow-y: auto;
  }
  li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto 190px;
    align-items: center;
    gap: 10px;
    padding: 8px 0;
    border-bottom: 1px solid var(--border);
  }
  li:last-child {
    border-bottom: 0;
  }
  .about {
    min-width: 0;
  }
  .who,
  .sample {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .count {
    margin-left: 6px;
    font-size: 12px;
    color: var(--muted);
  }
  .sample {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    font-size: 12px;
    color: var(--muted);
  }
  .listen {
    padding: 2px 8px;
  }
  .warning {
    margin: 0;
    padding: 6px 10px;
    color: var(--warn-fg);
    background: var(--warn-bg);
    border-radius: 6px;
    overflow-wrap: anywhere;
  }
  .error {
    margin: 0;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
