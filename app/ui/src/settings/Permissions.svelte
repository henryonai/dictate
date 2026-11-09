<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorText } from "../lib/api";
  import type { PermissionKind, PermissionState, PermissionsView } from "../lib/api";
  import { t } from "../lib/i18n";

  // The macOS permissions that dictation uses. Windows and Linux ask for none of them, and nothing is shown there.
  const KINDS: PermissionKind[] = ["microphone", "accessibility"];
  const POLL_MS = 1000;

  // The system's answer, or null before the first one. The person turns the permissions on in System Settings, in
  // another window, so this asks again every second for as long as the section is on screen.
  let status = $state<PermissionsView | null>(null);
  let actionError = $state("");
  let host = $state<HTMLElement>();
  let asking = false;

  const asked = $derived(status !== null && KINDS.some((kind) => status![kind] !== "not_needed"));

  async function refresh() {
    if (asking) return;
    asking = true;
    try {
      status = await api.permissionsStatus();
    } catch {
      // The section stays as it was; the next ask may work.
    } finally {
      asking = false;
    }
  }

  // Settings keeps every page in its window and hides the ones of the tabs that aren't selected: a page that nobody sees
  // has nothing to ask for. A window that is hidden or minimised says the same through the document.
  function onScreen(): boolean {
    return !document.hidden && host?.closest("[hidden]") == null;
  }

  onMount(() => {
    void refresh();
    const timer = setInterval(() => {
      if (onScreen()) void refresh();
    }, POLL_MS);
    return () => clearInterval(timer);
  });

  // The system shows its prompt for the microphone once. After a refusal the person has to turn it on in System
  // Settings, and the prompt would do nothing. Accessibility has no "not asked yet": either it is on or it isn't.
  const canGrant = (kind: PermissionKind, state: PermissionState) =>
    kind === "microphone" ? state === "not_determined" : state === "denied";
  const canOpenSettings = (state: PermissionState) => state === "denied";

  async function run(action: () => Promise<void>) {
    actionError = "";
    try {
      await action();
    } catch (e) {
      actionError = errorText(e);
    }
    void refresh();
  }

  const hint = (kind: PermissionKind, state: PermissionState) =>
    kind === "microphone" && state === "denied" ? t("audio.denied") : t(`permissions.${kind}_hint`);
</script>

<div bind:this={host}>
  {#if status && asked}
    {@const current = status}
    <section class="group" aria-labelledby="general-permissions">
      <h2 id="general-permissions">{t("permissions.title")}</h2>
      <ul class="permissions">
        {#each KINDS as kind (kind)}
          {@const state = current[kind]}
          <li aria-labelledby={`permission-${kind}`}>
            <div class="info">
              <span>
                <strong id={`permission-${kind}`}>{t(`permissions.${kind}`)}</strong>
                <span class="state {state}">{t(`permissions.state.${state}`)}</span>
              </span>
              <span class="hint">{hint(kind, state)}</span>
            </div>
            <div class="actions">
              {#if canGrant(kind, state)}
                <button type="button" class="primary" onclick={() => run(() => api.requestPermission(kind))}>
                  {t("permissions.grant")}
                </button>
              {/if}
              {#if canOpenSettings(state)}
                <button type="button" onclick={() => run(() => api.openPermissionSettings(kind))}>
                  {t("permissions.open_settings")}
                </button>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
      {#if actionError}
        <p class="error" role="alert">{actionError}</p>
      {/if}
      <p class="hint note">{t("permissions.rebuilt_note")}</p>
    </section>
  {/if}
</div>

<style>
  .permissions {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .permissions li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 0;
    border-bottom: 1px solid var(--border);
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
  .state {
    margin-left: 6px;
    color: var(--muted);
  }
  .state.granted {
    color: var(--ok);
  }
  .state.denied {
    color: var(--danger);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .note {
    margin: 8px 0 0;
  }
</style>
