<script lang="ts">
  import type { MeetingDetail, MeetingRow } from "../lib/api";
  import { t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import Menu from "./Menu.svelte";

  // The ⋯ menu of a recording: to do its work again, to rename its speakers, to open its folder, its media or its log, and to delete it.
  // Doing the work again is for a recording that is not at work: while a job runs, the choices of it are not there.
  let {
    store,
    row,
    detail,
    onrelease,
  }: {
    store: MeetingsStore;
    row: MeetingRow;
    detail: MeetingDetail | null;
    /** Called before the recording is moved to the Trash: the player lets go of the file, and gives back what puts it as it was. */
    onrelease: () => () => void;
  } = $props();

  const busy = $derived(row.live !== null);
  const hasTranscript = $derived((detail?.transcript.length ?? 0) > 0);
  const hasSpeakers = $derived((detail?.speakers.length ?? 0) > 0);
</script>

<Menu text="⋯" label={t("meetings.more")} glyph>
  {#snippet children(pick)}
    {#if !busy}
      <button type="button" disabled={!hasTranscript} onclick={() => pick(() => store.resummarize(row.id))}>{t("meetings.menu.resummarize")}</button>
      <button type="button" onclick={() => pick(() => store.rerun(row.id))}>{t("meetings.menu.rerun")}</button>
      <button type="button" disabled={!hasSpeakers} onclick={() => pick(() => store.openRename())}>{t("meetings.menu.rename")}</button>
      <hr />
    {/if}
    <button type="button" onclick={() => pick(() => store.openFolder(row.id))}>{t("meetings.menu.folder")}</button>
    {#if detail?.media}
      <button type="button" onclick={() => pick(() => store.openMedia(row.id))}>{t("meetings.menu.media")}</button>
    {/if}
    <button type="button" onclick={() => pick(() => store.showLog(row.id))}>{t("meetings.menu.log")}</button>
    <hr />
    <button type="button" class="danger" onclick={() => pick(() => store.remove(row.id, onrelease))}>{t("meetings.menu.delete")}</button>
  {/snippet}
</Menu>
