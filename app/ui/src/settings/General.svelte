<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorText } from "../lib/api";
  import type { Theme, UiLanguage } from "../lib/api";
  import { t } from "../lib/i18n";
  import { recordHotkey } from "../lib/recording";
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

  // THIRD_PARTY_NOTICES.md of the repository: Loah's licenses, and those of what it includes (FFmpeg and the others).
  const NOTICES_URL = "https://github.com/henryonai/loah/blob/main/THIRD_PARTY_NOTICES.md";

  // The section of the README about upgrading: what is copied, what to re-point, and how to remove the old app.
  const UPGRADE_URL = "https://github.com/henryonai/loah#upgrading-from-dictate";

  const UI_LANGUAGES: UiLanguage[] = ["system", "en", "vi"];
  const THEMES: Theme[] = ["system", "light", "dark"];
  const DICTATION_LANGUAGES = ["en-US", "vi-VN", "ja-JP", "zh-CN", "ko-KR", "fr-FR", "de-DE", "es-ES"];

  let folderError = $state("");
  let licensesError = $state("");
  // The old app is still on this machine (running, or set to start at login). Asked when the page is shown and again when the window
  // gets the focus back (a person quits it, or removes it, in another window), so it is true to the moment; where the answer can't be
  // had the notice stays as it was, and nothing else says so.
  let dictateStillPresent = $state(false);
  let upgradeError = $state("");

  function lookForDictate() {
    api
      .dictateStillPresent()
      .then((present) => (dictateStillPresent = present))
      .catch(() => {});
  }

  onMount(() => {
    lookForDictate();
    window.addEventListener("focus", lookForDictate);
    return () => window.removeEventListener("focus", lookForDictate);
  });

  const dictationLanguages = () => DICTATION_LANGUAGES.map((code) => ({ value: code, label: t(`lang.${code}`) }));

  // While the recorder listens, the shortcut that is set must not start a dictation. If the Rust side can't be asked, the
  // recorder works as well; the pause ends there by itself.
  function pauseHotkey(active: boolean) {
    api.hotkeyRecording(active).catch(() => {});
  }

  // The notices of third-party licenses are a page of the repository: they open in the system browser, and the app's own window
  // stays where it is.
  function openLicenses(event: MouseEvent) {
    event.preventDefault();
    licensesError = "";
    api.openUrl(NOTICES_URL).catch((e: unknown) => (licensesError = errorText(e)));
  }

  function openUpgradeGuide() {
    upgradeError = "";
    api.openUrl(UPGRADE_URL).catch((e: unknown) => (upgradeError = errorText(e)));
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

{#if dictateStillPresent}
  <div class="notice">
    <p role="status">{t("general.dictate_still_installed")}</p>
    <button type="button" onclick={openUpgradeGuide}>{t("general.dictate_still_installed_button")}</button>
    {#if upgradeError}
      <p class="error" role="alert">{upgradeError}</p>
    {/if}
  </div>
{/if}

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
  <!-- Saved here; Rust tells every window (`theme-changed`), this one too, and each applies it at once. -->
  <Field label={t("general.theme")}>
    <select value={view.theme} onchange={(event) => store.update({ theme: event.currentTarget.value as Theme })}>
      {#each THEMES as theme (theme)}
        <option value={theme}>{t(`general.theme.${theme}`)}</option>
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
  <!-- The recording has one shortcut, which is not set here: it is shown so that it is known. -->
  <div class="field">
    <label for="record-hotkey">{t("general.record_hotkey")}</label>
    <span class="field-control"><input id="record-hotkey" type="text" readonly value={recordHotkey()} /></span>
  </div>
  <p class="hint">{t("general.record_hotkey_hint")}</p>
  <button type="button" onclick={openMeetingsFolder}>{t("general.open_meetings")}</button>
  {#if folderError}
    <p class="error">{folderError}</p>
  {/if}
</section>

<section class="group" aria-labelledby="general-about">
  <h2 id="general-about">{t("general.about")}</h2>
  <p class="hint">{t("general.about_hint")}</p>
  <a href={NOTICES_URL} onclick={openLicenses}>{t("general.licenses")}</a>
  {#if licensesError}
    <p class="error">{licensesError}</p>
  {/if}
</section>

<style>
  .notice {
    margin-bottom: 16px;
    padding: 10px 14px;
    color: var(--warn-fg);
    background: var(--warn-bg);
    border-radius: 6px;
  }
  .notice p {
    margin: 0 0 8px;
    overflow-wrap: anywhere;
  }
</style>
