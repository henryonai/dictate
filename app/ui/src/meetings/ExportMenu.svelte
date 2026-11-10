<script lang="ts">
  import type { MeetingDetail } from "../lib/api";
  import { Flash } from "../lib/flash.svelte";
  import { t } from "../lib/i18n";
  import { transcriptText } from "../lib/meeting-text";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import Menu from "./Menu.svelte";

  // "Export": the Markdown of the recording, saved in a file or put on the clipboard, and the transcript on its own. It is off while
  // the recording has neither a summary nor a transcript.
  let { store, detail }: { store: MeetingsStore; detail: MeetingDetail } = $props();

  const id = $derived(detail.row.id);
  const hasTranscript = $derived(detail.transcript.length > 0);

  // The menu has closed by the time a copy is done: its button says that it was.
  const copied = new Flash();
  $effect(() => () => copied.clear());

  async function copy(done: Promise<boolean>) {
    if (await done) copied.show();
  }
</script>

<Menu text={copied.on ? t("meetings.copied") : t("meetings.export")} label={t("meetings.export")} disabled={!detail.summary && !hasTranscript}>
  {#snippet children(pick)}
    <button type="button" onclick={() => pick(() => store.exportMarkdown(id))}>{t("meetings.export.save")}</button>
    <button type="button" onclick={() => pick(() => copy(store.copyMarkdown(id)))}>{t("meetings.export.copy_all")}</button>
    <button type="button" disabled={!hasTranscript} onclick={() => pick(() => copy(store.copy(transcriptText(detail.transcript))))}>
      {t("meetings.export.copy_transcript")}
    </button>
  {/snippet}
</Menu>
