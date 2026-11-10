<script lang="ts">
  import type { ActionView, MeetingDetail } from "../lib/api";
  import { Flash } from "../lib/flash.svelte";
  import { t } from "../lib/i18n";
  import { checklistText } from "../lib/meeting-text";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import Ask from "./Ask.svelte";
  import Inline from "./Inline.svelte";

  // The summary of a recording in its parts: what it was about, the key points, the decisions, the action items (with a box to tick
  // for each), the open questions and the chapters, and then the questions that are asked of it. Every text in it is a model's: it is
  // shown as text, and its stamps seek the player.
  let {
    store,
    detail,
    canSeek,
    onseek,
  }: {
    store: MeetingsStore;
    detail: MeetingDetail;
    /** There is a player to seek: the chapters are off without one, and so are the links of the texts. */
    canSeek: boolean;
    onseek: (seconds: number) => void;
  } = $props();

  const id = $derived(detail.row.id);
  const summary = $derived(detail.summary);
  /** What the texts do with a stamp: seek, when there is a player. */
  const seek = $derived(canSeek ? onseek : undefined);

  // A tick shows at once and is written meanwhile; the detail that is read again after the write says the same, and then it is the
  // detail's again. A write that fails takes its tick back.
  let ticks = $state<Record<string, boolean>>({});
  const keyOf = (index: number) => `${id}:${index}`;
  const actions = $derived(
    (summary?.actions ?? []).map((action) => ({ ...action, done: keyOf(action.index) in ticks ? ticks[keyOf(action.index)] : action.done })),
  );

  $effect(() => {
    for (const action of summary?.actions ?? []) {
      const key = keyOf(action.index);
      if (key in ticks && ticks[key] === action.done) delete ticks[key];
    }
  });

  /** The writes go one after the other, so that the last tick is the one that is left in the file. */
  let writes: Promise<void> = Promise.resolve();

  function toggle(action: ActionView, done: boolean) {
    const key = keyOf(action.index);
    const target = id;
    ticks[key] = done;
    writes = writes.then(async () => {
      if (!(await store.setTask(target, action.index, done))) delete ticks[key];
    });
  }

  const copied = new Flash();
  $effect(() => () => copied.clear());

  async function copyChecklist() {
    if (await store.copy(checklistText(actions))) copied.show();
  }
</script>

<div class="pane">
  {#if !summary}
    <section class="none">
      <p class="title">{t("meetings.summary.none")}</p>
      {#if store.readiness?.ai === false}
        <p class="hint">{t("meetings.summary.add_api")}</p>
        <button type="button" onclick={() => store.openSettings("accounts")}>{t("meetings.errorcard.open_api")}</button>
      {:else if store.readiness?.ai && detail.transcript.length > 0}
        <button type="button" class="primary" onclick={() => store.resummarize(id)}>{t("meetings.summary.now")}</button>
      {/if}
    </section>
  {:else}
    {#if summary.summary}
      <section>
        <h2>{t("meetings.summary.summary")}</h2>
        <Inline text={summary.summary} onseek={seek} />
      </section>
    {/if}

    {#if summary.keyPoints}
      <section>
        <h2>{t("meetings.summary.key_points")}</h2>
        <Inline text={summary.keyPoints} onseek={seek} />
      </section>
    {/if}

    {#if summary.decisions.length > 0}
      <section>
        <h2>{t("meetings.summary.decisions")}</h2>
        <ul class="bullets">
          {#each summary.decisions as decision, index (index)}
            <li><Inline text={decision} onseek={seek} /></li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if actions.length > 0}
      <section>
        <div class="head">
          <h2>{t("meetings.summary.actions")}</h2>
          <button type="button" class="small" onclick={copyChecklist}>{copied.on ? t("meetings.copied") : t("meetings.summary.copy_checklist")}</button>
        </div>
        <ul class="actions">
          {#each actions as action (action.index)}
            <li>
              <label class:done={action.done}>
                <input type="checkbox" checked={action.done} onchange={(event) => toggle(action, event.currentTarget.checked)} />
                <span class="text">{action.text}</span>
              </label>
              {#if action.owner}<span class="owner">{action.owner}</span>{/if}
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if summary.questions.length > 0}
      <section>
        <h2>{t("meetings.summary.questions")}</h2>
        <ul class="bullets">
          {#each summary.questions as question, index (index)}
            <li><Inline text={question} onseek={seek} /></li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if summary.timeline.length > 0}
      <section>
        <h2>{t("meetings.summary.chapters")}</h2>
        <div class="chapters">
          {#each summary.timeline as mark, index (index)}
            <button type="button" class="chip" disabled={!canSeek} onclick={() => onseek(mark.seconds)}>
              <span class="stamp">{mark.label}</span>
              {mark.text}
            </button>
          {/each}
        </div>
      </section>
    {/if}

    {#each summary.other as section, index (index)}
      <section>
        {#if section.heading}<h2>{section.heading}</h2>{/if}
        <Inline text={section.body} onseek={seek} />
      </section>
    {/each}
  {/if}

  {#if detail.transcript.length > 0}
    <Ask {store} {id} onseek={seek} />
  {/if}
</div>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  section h2 {
    margin-bottom: 4px;
  }
  .none .title {
    margin: 0 0 6px;
    font-weight: 600;
  }
  .none .hint {
    margin: 0 0 8px;
  }
  .bullets {
    margin: 0;
    padding-left: 18px;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .head h2 {
    margin: 0;
  }
  .small {
    padding: 1px 8px;
    font-size: 12px;
  }
  .actions {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .actions li {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 8px;
    padding: 2px 0;
  }
  label {
    display: flex;
    align-items: flex-start;
    gap: 6px;
  }
  label input {
    margin-top: 2px;
  }
  label.done {
    color: var(--muted);
  }
  label.done .text {
    text-decoration: line-through;
  }
  .text {
    overflow-wrap: anywhere;
    user-select: text;
  }
  .owner {
    flex: none;
    padding: 1px 8px;
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    background: var(--accent-soft);
    border-radius: 999px;
  }
  .chapters {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
  }
  .chip {
    padding: 2px 10px;
    text-align: left;
    border-radius: 999px;
    overflow-wrap: anywhere;
  }
  .stamp {
    margin-right: 6px;
    font-variant-numeric: tabular-nums;
    color: var(--accent);
  }
  .chip:disabled .stamp {
    color: inherit;
  }
</style>
