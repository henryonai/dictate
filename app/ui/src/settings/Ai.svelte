<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { EditPresetView } from "../lib/api";
  import { t } from "../lib/i18n";
  import type { SettingsStore } from "../lib/settings-store.svelte";
  import Field from "./Field.svelte";
  import Toggle from "./Toggle.svelte";

  // `store` has loaded its view before this page is shown.
  let { store }: { store: SettingsStore } = $props();
  const view = $derived(store.view!);

  // The presets come from the Rust side, which sends their instructions to the model; the UI only names them.
  let presets = $state<EditPresetView[] | null>(null);

  onMount(() => {
    api
      .editPresets()
      .then((list) => (presets = list))
      .catch(() => (presets = []));
  });

  /** The ids to offer: the presets, and the stored one when it isn't among them, so that it doesn't silently change. */
  function presetIds(list: EditPresetView[], stored: string): string[] {
    const ids = list.map((preset) => preset.id);
    return ids.includes(stored) ? ids : [...ids, stored];
  }

  /** The title in the UI language; an id nobody has a title for shows as itself. */
  function title(id: string): string {
    const text = t(`presets.${id}`);
    return text === `presets.${id}` ? id : text;
  }
</script>

<section class="group" aria-labelledby="ai-edit">
  <h2 id="ai-edit">{t("ai.edit")}</h2>
  {#if presets}
    <Field label={t("ai.default_preset")}>
      <select
        value={view.default_edit_preset}
        onchange={(event) => store.update({ default_edit_preset: event.currentTarget.value })}
      >
        {#each presetIds(presets, view.default_edit_preset) as id (id)}
          <option value={id}>{title(id)}</option>
        {/each}
      </select>
    </Field>
  {/if}
  <Toggle
    label={t("ai.auto_refine")}
    checked={view.auto_refine}
    onchange={(checked) => store.update({ auto_refine: checked })}
  />
</section>

<section class="group" aria-labelledby="ai-summary">
  <h2 id="ai-summary">{t("ai.summary")}</h2>
  <Field label={t("ai.summary_model")}>
    <input
      type="text"
      value={view.summary_model}
      onchange={(event) => store.update({ summary_model: event.currentTarget.value.trim() })}
    />
  </Field>
</section>

<section class="group">
  <h2 id="ai-vocabulary">{t("ai.vocabulary")}</h2>
  <textarea
    aria-labelledby="ai-vocabulary"
    value={view.vocabulary}
    onchange={(event) => store.update({ vocabulary: event.currentTarget.value })}
  ></textarea>
  <p class="hint">{t("ai.vocabulary_hint")}</p>
</section>
