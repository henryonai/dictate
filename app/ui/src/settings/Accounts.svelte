<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorText } from "../lib/api";
  import type { Account, AiProvider, TranscribePreset, TranscribePresetView } from "../lib/api";
  import { t } from "../lib/i18n";
  import type { SettingsStore } from "../lib/settings-store.svelte";
  import CheckButton from "./CheckButton.svelte";
  import ChoiceOrText from "./ChoiceOrText.svelte";
  import Field from "./Field.svelte";
  import KeyField from "./KeyField.svelte";

  // `store` has loaded its view before this page is shown.
  let { store }: { store: SettingsStore } = $props();
  const view = $derived(store.view!);

  const AI_PROVIDERS: AiProvider[] = ["anthropic", "openai"];
  const SERVICES: TranscribePreset[] = ["openrouter", "openai", "groq", "custom"];

  // The models of each service and the page that hands out its keys come from the Rust side.
  let presets = $state<TranscribePresetView[] | null>(null);
  const preset = $derived(presets?.find((p) => p.id === view.transcribe_preset));

  onMount(() => {
    api
      .transcribePresets()
      .then((list) => (presets = list))
      .catch(() => (presets = []));
  });

  // What is typed in the key fields and not saved yet.
  let aiDraft = $state("");
  let transcribeDraft = $state("");
  let keyPageError = $state("");

  /** Saves what is typed in a key field, if anything. Whether there is no failure. */
  async function saveDraft(account: Account): Promise<boolean> {
    const draft = account === "ai" ? aiDraft : transcribeDraft;
    if (draft.trim() === "") return true;
    const saved = await store.setSecret(account, draft);
    if (saved) {
      if (account === "ai") aiDraft = "";
      else transcribeDraft = "";
    }
    return saved;
  }

  /** A key that was typed counts: saving it first is what the person meant by pressing Check. */
  async function readyToCheck(account: Account) {
    if (!(await saveDraft(account))) throw new Error(store.error);
    await store.settled();
  }

  async function checkAi(): Promise<string> {
    await readyToCheck("ai");
    await api.checkAi();
    return t("accounts.check_ai_ok", { model: view.ai_model });
  }

  async function checkTranscribe(): Promise<string> {
    await readyToCheck("transcribe");
    await api.checkTranscribe();
    return t("accounts.check_transcribe_ok", { model: view.transcribe_model });
  }

  function openKeyPage(event: MouseEvent, url: string) {
    // The page opens in the system browser; the app's own window stays where it is.
    event.preventDefault();
    keyPageError = "";
    api.openUrl(url).catch((e: unknown) => (keyPageError = errorText(e)));
  }

  // What each check depends on. A typed key counts as soon as it is typed.
  const aiConfig = $derived(
    JSON.stringify([view.ai_provider, view.ai_base_url, view.ai_model, view.has_ai_key, aiDraft]),
  );
  const transcribeConfig = $derived(
    JSON.stringify([
      view.transcribe_preset,
      view.transcribe_base_url,
      view.transcribe_model,
      view.has_transcribe_key,
      view.shares_openrouter_key,
      transcribeDraft,
    ]),
  );

  const modelOptions = (models: string[]) => models.map((model) => ({ value: model, label: model }));
</script>

<section class="group" aria-labelledby="accounts-ai">
  <h2 id="accounts-ai">{t("accounts.ai")}</h2>
  <Field label={t("accounts.provider")}>
    <select
      value={view.ai_provider}
      onchange={(event) => store.setAiProvider(event.currentTarget.value as AiProvider)}
    >
      {#each AI_PROVIDERS as provider (provider)}
        <option value={provider}>{t(`provider.${provider}`)}</option>
      {/each}
    </select>
  </Field>
  <Field label={t("accounts.base_url")}>
    <input
      type="text"
      value={view.ai_base_url}
      spellcheck="false"
      onchange={(event) => store.update({ ai_base_url: event.currentTarget.value.trim() })}
    />
  </Field>
  <Field label={t("accounts.model")}>
    <input
      type="text"
      value={view.ai_model}
      spellcheck="false"
      onchange={(event) => store.update({ ai_model: event.currentTarget.value.trim() })}
    />
  </Field>
  <KeyField
    label={t("accounts.api_key")}
    saved={view.has_ai_key}
    bind:draft={aiDraft}
    onsave={() => saveDraft("ai")}
    onremove={() => store.setSecret("ai", "")}
  />
  <CheckButton run={checkAi} config={aiConfig} />
  <p class="hint">{t("accounts.ai_footer")}</p>
</section>

<section class="group" aria-labelledby="accounts-transcribe">
  <h2 id="accounts-transcribe">{t("accounts.transcribe")}</h2>
  <Field label={t("accounts.service")}>
    <select
      value={view.transcribe_preset}
      onchange={(event) => store.setTranscribePreset(event.currentTarget.value as TranscribePreset)}
    >
      {#each SERVICES as service (service)}
        <option value={service}>{t(`service.${service}`)}</option>
      {/each}
    </select>
  </Field>
  <Field label={t("accounts.base_url")}>
    <input
      type="text"
      value={view.transcribe_base_url}
      spellcheck="false"
      onchange={(event) => store.update({ transcribe_base_url: event.currentTarget.value.trim() })}
    />
  </Field>
  {#if presets}
    <ChoiceOrText
      label={t("accounts.file_model")}
      customLabel={t("accounts.file_model_name")}
      options={modelOptions(preset?.models ?? [])}
      value={view.transcribe_model}
      onchange={(model) => store.update({ transcribe_model: model })}
    />
    <ChoiceOrText
      label={t("accounts.dictation_model")}
      customLabel={t("accounts.dictation_model_name")}
      options={modelOptions(preset?.models ?? [])}
      value={view.dictation_model}
      onchange={(model) => store.update({ dictation_model: model })}
    />
  {/if}
  <KeyField
    label={t("accounts.api_key")}
    saved={view.has_transcribe_key}
    bind:draft={transcribeDraft}
    onsave={() => saveDraft("transcribe")}
    onremove={() => store.setSecret("transcribe", "")}
  />
  {#if view.transcribe_preset === "openrouter"}
    <p class="hint">{t("accounts.key_optional")}</p>
  {/if}
  {#if view.shares_openrouter_key}
    <p class="hint">{t("accounts.shares_key")}</p>
  {/if}
  <CheckButton run={checkTranscribe} config={transcribeConfig} />
  {#if preset?.key_url}
    <a href={preset.key_url} onclick={(event) => openKeyPage(event, preset.key_url!)}>{t("accounts.get_key")}</a>
  {/if}
  {#if keyPageError}
    <p class="error">{keyPageError}</p>
  {/if}
  {#if view.transcribe_preset === "openrouter"}
    <p class="hint">{t("accounts.footer_openrouter")}</p>
  {/if}
  <p class="hint">{t("accounts.footer_models")}</p>
</section>

<p class="hint keys-note">{t("accounts.keys_note")}</p>
