<script lang="ts">
  import { api, errorText } from "../lib/api";
  import type { UiLanguage } from "../lib/api";
  import { t } from "../lib/i18n";
  import type { SettingsStore } from "../lib/settings-store.svelte";
  import ChoiceOrText from "./ChoiceOrText.svelte";
  import Field from "./Field.svelte";
  import HotkeyRecorder from "./HotkeyRecorder.svelte";
  import Permissions from "./Permissions.svelte";
  import Toggle from "./Toggle.svelte";

  // `store` has loaded its view before this page is shown.
  let { store }: { store: SettingsStore } = $props();
  const view = $derived(store.view!);

  // What the Rust side says about the shortcut. Before it has said, the shortcut is as the settings write it.
  const hotkey = $derived(store.hotkey);
  const hotkeyLabel = $derived(hotkey?.label ?? view.hotkey);
  // Where the desktop owns the shortcut, or offers none, changing it here would do nothing.
  const hotkeyChangeable = $derived(hotkey?.backend !== "portal" && hotkey?.error !== "hotkey.wayland_unavailable");

  const UI_LANGUAGES: UiLanguage[] = ["system", "en", "vi"];
  const DICTATION_LANGUAGES = ["en-US", "vi-VN", "ja-JP", "zh-CN", "ko-KR", "fr-FR", "de-DE", "es-ES"];

  let folderError = $state("");

  const dictationLanguages = () => DICTATION_LANGUAGES.map((code) => ({ value: code, label: t(`lang.${code}`) }));

  // While the recorder listens, the shortcut that is set must not start a dictation. If the Rust side can't be asked, the
  // recorder works as well; the pause ends there by itself.
  function pauseHotkey(active: boolean) {
    api.hotkeyRecording(active).catch(() => {});
  }

  async function openMeetingsFolder() {
    folderError = "";
    try {
      await api.openMeetingsFolder();
    } catch (e) {
      folderError = errorText(e);
    }
  }
</script>

<section class="group" aria-labelledby="general-interface">
  <h2 id="general-interface">{t("general.interface")}</h2>
  <Field label={t("general.ui_language")}>
    <select
      value={view.ui_language}
      onchange={(event) => store.update({ ui_language: event.currentTarget.value as UiLanguage })}
    >
      {#each UI_LANGUAGES as language (language)}
        <option value={language}>{t(`general.ui_language.${language}`)}</option>
      {/each}
    </select>
  </Field>
  <Toggle
    label={t("general.launch_at_login")}
    hint={t("general.launch_at_login_hint")}
    checked={view.launch_at_login}
    onchange={(checked) => store.update({ launch_at_login: checked })}
  />
</section>

<section class="group" aria-labelledby="general-dictation">
  <h2 id="general-dictation">{t("general.dictation")}</h2>
  <ChoiceOrText
    label={t("general.dictation_language")}
    customLabel={t("general.dictation_language_custom")}
    options={dictationLanguages()}
    value={view.dictation_language}
    onchange={(language) => store.update({ dictation_language: language })}
  />
  <HotkeyRecorder
    label={hotkeyLabel}
    canChange={hotkeyChangeable}
    onrecord={(accelerator) => store.setHotkey(accelerator)}
    onlistening={pauseHotkey}
  />
  {#if hotkey?.error === "hotkey.wayland_unavailable"}
    <p class="hint">{t(hotkey.error)}</p>
  {:else if hotkey?.error}
    <p class="error" role="alert">{t(hotkey.error)}</p>
  {:else if hotkey?.backend === "portal"}
    <p class="hint">{t("general.hotkey_portal_hint")}</p>
  {/if}
  <p class="hint">{t("general.hotkey_hint")}</p>
  <Field label={t("general.mode")}>
    <select
      value={view.push_to_talk ? "hold" : "toggle"}
      onchange={(event) => store.update({ push_to_talk: event.currentTarget.value === "hold" })}
    >
      <option value="toggle">{t("general.mode.toggle")}</option>
      <option value="hold">{t("general.mode.hold")}</option>
    </select>
  </Field>
  <Toggle
    label={t("general.auto_write")}
    hint={t("general.auto_write_hint")}
    checked={view.auto_write}
    onchange={(checked) => store.update({ auto_write: checked })}
  />
  <Toggle
    label={t("general.save_recordings")}
    checked={view.save_recordings}
    onchange={(checked) => store.update({ save_recordings: checked })}
  />
</section>

<!-- macOS only: it shows nothing where the system asks for no permission. -->
<Permissions />

<section class="group" aria-labelledby="general-meetings">
  <h2 id="general-meetings">{t("general.meetings")}</h2>
  <button type="button" onclick={openMeetingsFolder}>{t("general.open_meetings")}</button>
  {#if folderError}
    <p class="error">{folderError}</p>
  {/if}
</section>
