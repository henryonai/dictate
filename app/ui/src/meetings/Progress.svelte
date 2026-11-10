<script lang="ts">
  import type { JobView, MeetingDetail, MeetingKind, StepName } from "../lib/api";
  import { formatClock, timeLeft } from "../lib/days";
  import { t } from "../lib/i18n";
  import type { MeetingsStore } from "../lib/meetings-store.svelte";

  // What a job is doing: a card for each step, how long the step has run and how long it has left, and below them the
  // transcript so far. The job and the detail come from the store; the clock is this panel's own.
  let {
    store,
    job,
    kind,
    detail,
  }: { store: MeetingsStore; job: JobView; kind: MeetingKind; detail: MeetingDetail | null } = $props();

  const CARD: Record<StepName, string> = {
    download: "meetings.step.download",
    audio: "meetings.step.audio",
    transcript: "meetings.step.transcript",
    summary: "meetings.step.summary",
  };
  /** The title of each step as a status ("Transcribing"). */
  const STATUS: Record<StepName, string> = {
    download: "meetings.status.download",
    audio: "meetings.status.audio",
    transcript: "meetings.status.transcript",
    summary: "meetings.status.summary",
  };

  /** A link is downloaded first. */
  const steps = $derived<StepName[]>(
    kind === "link" || job.step === "download"
      ? ["download", "audio", "transcript", "summary"]
      : ["audio", "transcript", "summary"],
  );
  /** -1 while the job only waits: every card is waiting then. */
  const active = $derived(job.step === null ? -1 : steps.indexOf(job.step));

  const percent = $derived(job.progress === null ? null : Math.round(Math.min(1, Math.max(0, job.progress)) * 100));

  // The status line is refreshed every second.
  let now = $state(Date.now());
  $effect(() => {
    const clock = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(clock);
  });

  const elapsedS = $derived(Math.max(0, (now - job.stepStartedMs) / 1000));
  const left = $derived(job.progress === null ? null : timeLeft(elapsedS, job.progress));
  const status = $derived.by(() => {
    if (job.step === null) return t("meetings.progress.queued");
    const line = t("meetings.progress.line", { step: t(STATUS[job.step]), time: formatClock(elapsedS) });
    return left === null ? line : line + t("meetings.progress.left", { time: formatClock(left) });
  });
</script>

<section class="progress">
  <ol class="steps" aria-label={t("meetings.progress.label")}>
    {#each steps as step, index (step)}
      {@const state = index < active ? "done" : index === active ? "active" : "waiting"}
      <li class="step {state}" data-state={state} aria-current={state === "active" ? "step" : undefined}>
        <span class="name">{t(CARD[step])}</span>
        <span class="caption">
          {#if state === "done"}
            <span aria-hidden="true">✓</span> {t("meetings.step.done")}
          {:else if state === "active"}
            {#if percent === null}
              <span class="spinner" aria-hidden="true"></span> {t("meetings.step.running")}
            {:else}
              {percent}%
            {/if}
          {:else}
            {t("meetings.step.waiting")}
          {/if}
        </span>
        {#if state === "active" && percent !== null}
          <progress max="100" value={percent} aria-label={t(CARD[step])}></progress>
        {/if}
      </li>
    {/each}
  </ol>

  <p class="status">{status}</p>
  <p class="hint">{t("meetings.progress.close_note")}</p>
  <button type="button" onclick={() => store.cancel(job.id)}>{t("common.cancel")}</button>

  {#if detail && detail.transcript.length > 0}
    <!-- A region that scrolls takes the focus, so that the keyboard can scroll it. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <section class="preview" tabindex="0" aria-labelledby="preview-title">
      <h3 id="preview-title">{detail.transcriptIsLive ? t("meetings.transcript.live") : t("meetings.transcript")}</h3>
      {#each detail.transcript as line, index (index)}
        <p class="line">
          <span class="stamp">{line.stamp}</span>
          {#if line.speaker}<strong class="speaker">{line.speaker}</strong>{/if}
          <span class="text">{line.text}</span>
        </p>
      {/each}
    </section>
  {/if}
</section>

<style>
  .progress {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
  }
  .steps {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
    gap: 8px;
    width: 100%;
    margin: 0 0 4px;
    padding: 0;
    list-style: none;
  }
  .step {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px 12px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
  }
  .step.done {
    color: var(--ok);
  }
  .step.active {
    color: var(--warn-fg);
    background: var(--warn-bg);
  }
  .step.waiting {
    color: var(--muted);
  }
  .name {
    font-weight: 600;
  }
  .caption {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
  }
  progress {
    width: 100%;
    height: 6px;
  }
  .spinner {
    display: inline-block;
    width: 10px;
    height: 10px;
    border: 2px solid currentColor;
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
  .status {
    margin: 0;
    font-weight: 500;
  }
  .hint {
    margin: 0;
  }
  .preview {
    align-self: stretch;
    max-height: 340px;
    margin-top: 8px;
    padding: 10px 14px;
    overflow-y: auto;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
  }
  .preview h3 {
    margin: 0 0 8px;
    font-size: 12px;
    color: var(--muted);
  }
  .line {
    margin: 0 0 6px;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .stamp {
    margin-right: 6px;
    font-variant-numeric: tabular-nums;
    color: var(--muted);
  }
  .speaker {
    margin-right: 4px;
  }
</style>
