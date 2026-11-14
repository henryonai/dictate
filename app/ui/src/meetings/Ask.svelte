<script lang="ts">
  import { tick } from "svelte";
  import { errorMessage } from "../lib/meeting-errors";
  import { t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import Inline from "./Inline.svelte";

  // Questions about a recording: a field, and the questions of this session with their answers. Each question stands alone (the model
  // is not told the ones before it), and the history is in memory only: the store keeps it until the window closes.
  let { store, id, onseek }: { store: MeetingsStore; id: string; onseek?: (seconds: number) => void } = $props();

  const entries = $derived(store.asks.get(id) ?? []);
  const asking = $derived(store.asking(id));
  /** The settings say that no API is set up. While that is not known the field is on: the Rust side refuses what has no API. */
  const needsApi = $derived(store.readiness?.ai === false);

  let question = $state("");
  let field: HTMLInputElement | undefined = $state();
  let list: HTMLElement | undefined = $state();

  // The newest question is in sight: the list goes to its end when one is added.
  $effect(() => {
    void entries.length;
    if (list) list.scrollTop = list.scrollHeight;
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const text = question.trim();
    if (text === "" || asking || needsApi) return;
    const asked = id;
    question = "";
    await store.ask(asked, text);
    // A disabled field lets go of the focus: it has it back with the answer, unless the person has gone elsewhere.
    await tick();
    if (asked === id && document.activeElement === document.body) field?.focus();
  }
</script>

<section class="ask" aria-labelledby="ask-title">
  <h2 id="ask-title">{t("meetings.ask.title")}</h2>

  {#if entries.length > 0}
    <ol class="entries" bind:this={list}>
      {#each entries as entry, index (index)}
        <li>
          <p class="question">{entry.question}</p>
          {#if entry.error}
            <p class="error" role="alert">{t("meetings.ask.error", { message: errorMessage(entry.error) })}</p>
            {#if entry.error.kind === "Api"}
              <button type="button" onclick={() => store.openSettings("accounts")}>{t("meetings.errorcard.open_api")}</button>
            {/if}
          {:else if entry.answer === null}
            <span class="spinner" role="status" aria-label={t("meetings.ask.working")}></span>
          {:else}
            <div class="answer"><Inline text={entry.answer} {onseek} /></div>
          {/if}
        </li>
      {/each}
    </ol>
  {/if}

  {#if needsApi}
    <p class="hint">{t("meetings.error.no_ai")}</p>
    <button type="button" onclick={() => store.openSettings("accounts")}>{t("meetings.errorcard.open_api")}</button>
  {/if}

  <form onsubmit={submit}>
    <input
      type="text"
      bind:value={question}
      bind:this={field}
      disabled={asking || needsApi}
      placeholder={t("meetings.ask.placeholder")}
      aria-label={t("meetings.ask.label")}
      autocomplete="off"
    />
    <button type="submit" class="primary" disabled={asking || needsApi || question.trim() === ""}>{t("meetings.ask.submit")}</button>
  </form>
</section>

<style>
  /* The Ask tab: the questions and their answers scroll in the room that the tab has, and the field is at the bottom of it, in reach. */
  .ask {
    display: flex;
    flex: 1 1 auto;
    flex-direction: column;
    min-height: 0;
  }
  .entries {
    flex: 1 1 auto;
    min-height: 0;
    margin: 0 0 8px;
    padding: 0;
    overflow-y: auto;
    list-style: none;
  }
  li {
    margin: 0 0 10px;
  }
  .question {
    margin: 0 0 2px;
    font-weight: 600;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .answer {
    padding-left: 2px;
  }
  form {
    display: flex;
    flex: none;
    gap: 6px;
    margin-top: auto;
  }
  input {
    flex: 1;
    min-width: 0;
  }
  .spinner {
    display: inline-block;
    width: 12px;
    height: 12px;
    border: 2px solid var(--muted);
    border-right-color: transparent;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation-duration: 3s;
    }
  }
</style>
