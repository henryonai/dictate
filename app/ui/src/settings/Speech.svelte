<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorText } from "../lib/api";
  import type { Engine, ToolsView } from "../lib/api";
  import { t } from "../lib/i18n";
  import { ModelList } from "../lib/models.svelte";
  import type { SettingsStore } from "../lib/settings-store.svelte";
  import ChoiceOrText from "./ChoiceOrText.svelte";
  import Field from "./Field.svelte";
  import Toggle from "./Toggle.svelte";

  // `store` has loaded its view before this page is shown.
  let { store }: { store: SettingsStore } = $props();
  const view = $derived(store.view!);

  const ENGINES: Engine[] = ["whisper", "api"];
  const TRANSCRIPT_LANGUAGES = ["vi", "en", "ja", "ko", "zh"];

  // The list lives as long as the window does, so a download goes on while another tab is open.
  const models = new ModelList();
  onMount(() => {
    void models.start();
    return () => models.stop();
  });

  /** The model in use: the chosen one, else the one recommended for this machine. */
  const inUse = $derived(view.whisper_model ?? models.rows.find((row) => row.recommended)?.id ?? null);
  const inUseMissing = $derived(
    models.rows.length > 0 && inUse !== null && !models.rows.find((row) => row.id === inUse)?.downloaded,
  );

  const languages = () => [
    { value: "", label: t("speech.language.auto") },
    ...TRANSCRIPT_LANGUAGES.map((code) => ({ value: code, label: t(`lang.${code}`) })),
  ];

  function percent(done: number, total: number): number {
    return Math.min(100, Math.round((done / total) * 100));
  }

  // The tools that links need (yt-dlp downloads, ffmpeg reads what Dictate can't open itself). They are installed outside the app,
  // so they are looked for again when the window gets the focus back.
  let tools = $state.raw<ToolsView | null>(null);
  let toolsError = $state("");

  /** What `media_tools` says of each tool, in the order they are shown. */
  const toolRows = $derived(
    tools === null
      ? []
      : [
          { name: "ffmpeg", version: tools.ffmpeg, command: tools.ffmpegCommand },
          { name: "yt-dlp", version: tools.ytDlp, command: tools.ytDlpCommand },
        ],
  );

  async function lookForTools() {
    try {
      tools = await api.mediaTools();
      toolsError = "";
    } catch (e) {
      toolsError = errorText(e);
    }
  }

  /** `ffmpeg ✓ 7.1`; a tool that is there and does not say its version has none to show. */
  function found(name: string, version: string): string {
    return version === "unknown" ? t("speech.tools.found_unknown", { name }) : t("speech.tools.found", { name, version });
  }

  onMount(() => {
    void lookForTools();
    window.addEventListener("focus", lookForTools);
    return () => window.removeEventListener("focus", lookForTools);
  });
</script>

<section class="group" aria-labelledby="speech-dictation">
  <h2 id="speech-dictation">{t("speech.dictation")}</h2>
  <Field label={t("speech.dictation_engine")}>
    <select
      value={view.dictation_engine}
      onchange={(event) => store.update({ dictation_engine: event.currentTarget.value as Engine })}
    >
      {#each ENGINES as engine (engine)}
        <option value={engine}>{t(`speech.engine.${engine}`)}</option>
      {/each}
    </select>
  </Field>
</section>

<section class="group" aria-labelledby="speech-files">
  <h2 id="speech-files">{t("speech.files")}</h2>
  <Field label={t("speech.file_engine")}>
    <select
      value={view.file_engine}
      onchange={(event) => store.update({ file_engine: event.currentTarget.value as Engine })}
    >
      {#each ENGINES as engine (engine)}
        <option value={engine}>{t(`speech.engine.${engine}`)}</option>
      {/each}
    </select>
  </Field>
  {#if view.dictation_engine === "api" || view.file_engine === "api"}
    <p class="hint">{t("speech.engine_api_hint")}</p>
  {/if}
  <ChoiceOrText
    label={t("speech.language")}
    customLabel={t("speech.language_custom")}
    options={languages()}
    value={view.transcribe_language}
    onchange={(language) => store.update({ transcribe_language: language })}
  />
  <Toggle
    label={t("speech.diarize")}
    hint={t("speech.diarize_hint")}
    checked={view.diarize}
    onchange={(checked) => store.update({ diarize: checked })}
  />
  <div class="field" role="group" aria-labelledby="speech-tools">
    <span id="speech-tools">{t("speech.tools")}</span>
    <ul class="tools field-control">
      {#each toolRows as tool (tool.name)}
        <li>
          {#if tool.version === null}
            <span class="missing">{t("speech.tools.missing", { name: tool.name })}</span>
            <span class="hint">{t("speech.tools.run")} <code>{tool.command}</code></span>
          {:else}
            <span>{found(tool.name, tool.version)}</span>
          {/if}
        </li>
      {/each}
    </ul>
  </div>
  {#if toolsError}
    <p class="error">{toolsError}</p>
  {/if}
</section>

<section class="group" aria-labelledby="speech-models">
  <h2 id="speech-models">{t("models.heading")}</h2>
  <ul class="models">
    {#each models.rows as row (row.id)}
      {@const title = t(`models.title.${row.id}`)}
      {@const download = models.stateOf(row.id)}
      <li aria-label={title}>
        <input
          type="radio"
          name="whisper-model"
          aria-label={t("models.use", { name: title })}
          checked={inUse === row.id}
          onchange={() => store.update({ whisper_model: row.id })}
        />
        <div class="info">
          <span class="name">
            {title}
            {#if row.recommended}
              <span class="badge">{t("models.recommended")}</span>
            {/if}
          </span>
          <small class="hint">{t("models.size_mb", { n: row.size_mb })} · {t(`models.note.${row.id}`)}</small>
          {#if download.state === "failed"}
            <small class="error">{t("models.failed", { error: download.error })}</small>
          {/if}
        </div>
        <div class="actions">
          {#if download.state === "running"}
            <progress
              value={download.total ? download.done : undefined}
              max={download.total ?? undefined}
              aria-label={title}
            ></progress>
            {#if download.total}
              <span class="hint">{t("models.percent", { n: percent(download.done, download.total) })}</span>
            {/if}
            <button type="button" onclick={() => models.cancel(row.id)}>{t("common.cancel")}</button>
          {:else if download.state === "cancelling"}
            <button type="button" disabled>{t("common.cancelling")}</button>
          {:else if download.state === "failed"}
            <button type="button" onclick={() => models.download(row.id)}>{t("models.retry")}</button>
          {:else if !row.downloaded}
            <button type="button" onclick={() => models.download(row.id)}>{t("models.download")}</button>
          {/if}
          {#if row.downloaded}
            <button
              type="button"
              disabled={download.state === "running" || download.state === "cancelling"}
              onclick={() => models.remove(row.id)}
            >
              {t("models.delete")}
            </button>
          {/if}
        </div>
      </li>
    {/each}
  </ul>
  {#if models.error}
    <p class="error">{models.error}</p>
  {/if}
  {#if inUseMissing}
    <p class="warning">{t("models.selected_missing")}</p>
  {/if}
  <p class="disk">
    <span>{t("models.disk_usage")}</span>
    <span data-testid="disk-usage">{t("models.size_mb", { n: models.diskUsageMb })}</span>
  </p>
</section>

<style>
  .tools {
    margin: 0;
    padding: 0;
    list-style: none;
    text-align: right;
  }
  .tools .hint {
    display: block;
    margin: 0;
  }
  .tools code {
    font: 12px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    user-select: text;
    overflow-wrap: anywhere;
  }
  .missing {
    color: var(--danger);
  }
  .models {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .models li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 0;
    border-bottom: 1px solid var(--border);
  }
  .models li:last-child {
    border-bottom: 0;
  }
  .info {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }
  .info .hint {
    margin: 0;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  progress {
    width: 90px;
  }
  .warning {
    margin: 6px 0;
    padding: 6px 10px;
    color: var(--warn-fg);
    background: var(--warn-bg);
    border-radius: 6px;
  }
  .disk {
    display: flex;
    justify-content: space-between;
    margin: 8px 0 0;
  }
</style>
