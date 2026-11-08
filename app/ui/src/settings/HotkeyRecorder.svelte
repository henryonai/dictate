<script lang="ts">
  import { acceleratorFromKeydown } from "../lib/accelerator";
  import { errorText } from "../lib/api";
  import { t } from "../lib/i18n";

  // The shortcut as it reads, a button to change it and, while it listens, the keys that make the new one.
  // `onrecord` hands the new accelerator over and resolves with "" when it was taken, else with why it wasn't:
  // a key of the texts (`hotkey.taken`) or a sentence.
  let {
    label,
    canChange = true,
    onrecord,
  }: { label: string; canChange?: boolean; onrecord: (accelerator: string) => Promise<string> } = $props();

  const id = $props.id();

  let listening = $state(false);
  let failure = $state("");

  function start() {
    failure = "";
    listening = true;
  }

  function stop() {
    listening = false;
  }

  async function record(accelerator: string) {
    try {
      failure = await onrecord(accelerator);
    } catch (e) {
      failure = errorText(e);
    }
  }

  // While it listens, every key goes to the recorder: the window listens, not a field, because a key press can't be
  // told apart from typing anywhere else. Leaving the window ends it: the next keys are another app's.
  $effect(() => {
    if (!listening) return;
    const hear = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat) return;
      const bare = !event.ctrlKey && !event.altKey && !event.shiftKey && !event.metaKey;
      if (event.code === "Escape" && bare) {
        stop();
        return;
      }
      const accelerator = acceleratorFromKeydown(event);
      if (accelerator === null) return;
      stop();
      void record(accelerator);
    };
    window.addEventListener("keydown", hear, true);
    window.addEventListener("blur", stop);
    return () => {
      window.removeEventListener("keydown", hear, true);
      window.removeEventListener("blur", stop);
    };
  });
</script>

<div class="field">
  <label for={id}>{t("general.hotkey")}</label>
  <span class="field-control recorder">
    <input {id} type="text" readonly value={listening ? t("general.hotkey_listening") : label} />
    {#if canChange}
      <!-- One button, so that the focus stays on it. -->
      <button type="button" onclick={listening ? stop : start}>
        {listening ? t("general.hotkey_cancel") : t("general.hotkey_change")}
      </button>
    {/if}
  </span>
</div>
{#if listening}
  <p class="hint">{t("general.hotkey_listening_hint")}</p>
{/if}
{#if failure}
  <p class="error" role="alert">{t(failure)}</p>
{/if}

<style>
  .recorder {
    display: flex;
    gap: 8px;
  }
  .recorder input {
    flex: 1;
    min-width: 0;
  }
  .recorder button {
    flex-shrink: 0;
    width: auto;
  }
</style>
