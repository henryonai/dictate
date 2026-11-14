<script lang="ts">
  import type { ActionView, MeetingDetail } from "../lib/api";
  import { DETAIL_TABS, storedTab, storeTab } from "../lib/detail-tab";
  import type { DetailTab } from "../lib/detail-tab";
  import { Flash } from "../lib/flash.svelte";
  import { t } from "../lib/i18n";
  import { checklistText } from "../lib/meeting-text";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";
  import Ask from "./Ask.svelte";
  import Inline from "./Inline.svelte";
  import Overview from "./Overview.svelte";

  // The summary of a recording in four tabs: the Summary (what it was about, the key points, the decisions and the open questions), the
  // Actions (with a box to tick for each), the Chapters, and the Ask, where questions are asked of it. The tab that was chosen last is
  // the one that opens. Every text in it is a model's: it is shown as text, and its stamps seek the player.
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

  // ---- The tabs

  const uid = $props.id();
  const tabId = (tab: DetailTab) => `${uid}-tab-${tab}`;
  const panelId = `${uid}-panel`;

  /** The tabs that this recording has: the actions and the chapters are of a summary, and with no transcript there is nothing to ask. */
  const tabs = $derived(
    DETAIL_TABS.filter((tab) => (tab === "summary" ? true : tab === "ask" ? detail.transcript.length > 0 : summary !== null)),
  );
  /** The tab that was chosen last, which this recording may have no place for (the choice is left as it was then). */
  let chosen = $state<DetailTab>(storedTab());
  const active = $derived(tabs.includes(chosen) ? chosen : "summary");
  /** What a tab counts, when it counts: the action items and the chapters. Zero says nothing. */
  const counts = $derived<Partial<Record<DetailTab, number>>>({ actions: actions.length, chapters: summary?.timeline.length ?? 0 });

  function choose(tab: DetailTab) {
    chosen = tab;
    storeTab(tab);
  }

  let tablist: HTMLElement | undefined = $state();

  // The arrow keys go to the next tab and the one before it, round the ends; Home and End to the first and the last. The tab that they
  // reach is chosen and has the focus (the panel changes with it: there is no tab to look at before choosing it).
  function onkeydown(event: KeyboardEvent) {
    const at = tabs.indexOf(active);
    const to =
      event.key === "ArrowRight" ? (at + 1) % tabs.length
      : event.key === "ArrowLeft" ? (at - 1 + tabs.length) % tabs.length
      : event.key === "Home" ? 0
      : event.key === "End" ? tabs.length - 1
      : -1;
    if (to < 0) return;
    event.preventDefault();
    choose(tabs[to]);
    tablist?.querySelector<HTMLElement>(`[data-tab="${tabs[to]}"]`)?.focus();
  }

  // ---- A short recording

  /** Less than this is a short recording, in seconds. */
  const SHORT_SECONDS = 60;
  /** Less said than this is a short recording too, in words. */
  const SHORT_WORDS = 40;
  const words = $derived(detail.transcript.reduce((sum, line) => sum + (line.text.trim() === "" ? 0 : line.text.trim().split(/\s+/).length), 0));
  /**
   * There is not much to summarize: it is under a minute long, or under 40 words are said. A length that is not known counts for nothing,
   * and no transcript is not a short one (it may be on its way, or have failed).
   */
  const short = $derived(
    (detail.row.duration !== null && detail.row.duration < SHORT_SECONDS) || (detail.transcript.length > 0 && words < SHORT_WORDS),
  );
</script>

<div class="pane">
  <div class="tabs" role="tablist" aria-label={t("meetings.tabs.label")} bind:this={tablist}>
    {#each tabs as tab (tab)}
      <button
        type="button"
        role="tab"
        class="tab"
        id={tabId(tab)}
        data-tab={tab}
        aria-selected={active === tab}
        aria-controls={panelId}
        tabindex={active === tab ? 0 : -1}
        onclick={() => choose(tab)}
        {onkeydown}
      >
        {t(`meetings.tabs.${tab}`)}{#if counts[tab]}{" "}<span class="count">({counts[tab]})</span>{/if}
      </button>
    {/each}
  </div>

  <!-- A panel that scrolls is reached with the keyboard too (there may be nothing in it to focus); the Ask panel does not scroll as a
       whole: its answers do, and its field stays in reach. -->
  <div class="sections" class:fill={active === "ask"} role="tabpanel" id={panelId} aria-labelledby={tabId(active)} tabindex="0">
    {#if active === "summary"}
      {#if short}
        <p class="hint note">{t("meetings.summary.short")}</p>
      {/if}

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
            {#key summary.summary}
              <Overview text={summary.summary} onseek={seek} />
            {/key}
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

        {#each summary.other as section, index (index)}
          <section>
            {#if section.heading}<h2>{section.heading}</h2>{/if}
            <Inline text={section.body} onseek={seek} />
          </section>
        {/each}
      {/if}
    {:else if active === "actions"}
      <section>
        <div class="head">
          <h2>{t("meetings.summary.actions")}</h2>
          {#if actions.length > 0}
            <button type="button" class="small" onclick={copyChecklist}>{copied.on ? t("meetings.copied") : t("meetings.summary.copy_checklist")}</button>
          {/if}
        </div>
        {#if actions.length === 0}
          <p class="hint">{t("meetings.tabs.actions_none")}</p>
        {:else}
          <ul class="actions">
            {#each actions as action (action.index)}
              <li>
                <label class:done={action.done}>
                  <input type="checkbox" checked={action.done} onchange={(event) => toggle(action, event.currentTarget.checked)} />
                  <span class="text"><Inline inline text={action.text} onseek={seek} /></span>
                </label>
                {#if action.owner}<span class="owner">{action.owner}</span>{/if}
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {:else if active === "chapters"}
      <section>
        <h2>{t("meetings.summary.chapters")}</h2>
        {#if (summary?.timeline.length ?? 0) === 0}
          <p class="hint">{t("meetings.tabs.chapters_none")}</p>
        {:else}
          <ul class="chapters">
            {#each summary?.timeline ?? [] as mark, index (index)}
              <li>
                <button type="button" class="chip" disabled={!canSeek} onclick={() => onseek(mark.seconds)}>
                  <span class="stamp">{mark.label}</span>
                  {mark.text}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {:else}
      <Ask {store} {id} onseek={seek} />
    {/if}
  </div>
</div>

<style>
  .pane {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }
  /* The tabs do not scroll with their panel. There are four of them to fit a column that can be as narrow as 280 px: when the words are
     long (Vietnamese) the row scrolls sideways rather than break a label in two. */
  .tabs {
    display: flex;
    flex: none;
    gap: 2px;
    margin-bottom: 10px;
    overflow-x: auto;
    border-bottom: 1px solid var(--line);
  }
  .tab {
    flex: none;
    padding: 6px 8px;
    color: var(--muted);
    white-space: nowrap;
    background: transparent;
    border: 0;
    border-radius: 6px 6px 0 0;
  }
  .tab:hover {
    color: var(--ink);
  }
  /* The tab that is open: the ink, and a 2 px line of the accent under it, on the line of the row. */
  .tab[aria-selected="true"] {
    color: var(--ink);
    box-shadow: inset 0 -2px 0 var(--accent);
  }
  .tab:focus-visible {
    outline-offset: -2px;
  }
  .count {
    font-variant-numeric: tabular-nums;
  }
  /* The panel of a tab: its parts scroll. The text is for reading: 14 px, a line height of 1.55 (room for the marks of Vietnamese), and
     no line wider than 75 characters. */
  .sections {
    display: flex;
    flex: 1 1 auto;
    flex-direction: column;
    gap: 14px;
    min-height: 0;
    padding-bottom: 8px;
    overflow-y: auto;
    font-size: 14px;
    line-height: 1.55;
  }
  .sections:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  /* The Ask panel is not scrolled as a whole: its answers scroll, and the field stays under them. */
  .sections.fill {
    padding-bottom: 0;
    overflow-y: hidden;
  }
  .sections section {
    max-width: 75ch;
  }
  .note {
    margin: 0;
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
  .bullets li + li {
    margin-top: 4px;
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
  /* The time at the end of an item is a chip: the link that `Inline` makes of a stamp, drawn as a pill so that it reads as a place to go
     to. Without a player it is plain text again, as in `Inline`. The selectors name `.inline` (the box that `Inline` makes) so that they
     weigh more than the style of a stamp in `Inline` itself: the chip wins over it whichever of the two style sheets comes last. */
  .sections :global(.inline a[data-seek]) {
    padding: 0 7px;
    font-family: var(--mono);
    font-size: 0.85em;
    font-variant-numeric: tabular-nums;
    color: var(--ts);
    text-decoration: none;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 999px;
  }
  .sections :global(.inline a[data-seek]:hover) {
    background: var(--hl);
  }
  .sections :global(.inline a[data-inert]) {
    padding: 0;
    color: inherit;
    background: none;
    border: 0;
  }
  /* The chapters one under the other, each a chip. */
  .chapters {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .chip {
    padding: 2px 10px;
    text-align: left;
    border-radius: 999px;
    overflow-wrap: anywhere;
  }
  .stamp {
    margin-right: 6px;
    font-family: var(--mono);
    font-size: 0.92em;
    font-variant-numeric: tabular-nums;
    color: var(--ts);
  }
  .chip:disabled .stamp {
    color: inherit;
  }
</style>
