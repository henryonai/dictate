<script lang="ts">
  import type { MeetingRow, StepName } from "../lib/api";
  import { formatDuration } from "../lib/days";
  import { t } from "../lib/i18n";

  // One recording of the list: an icon for where it came from, its title, and a line that says how it stands.
  let { row, selected, onselect }: { row: MeetingRow; selected: boolean; onselect: (id: string) => void } = $props();

  /** The title of each step as a status ("Transcribing"). */
  const STATUS: Record<StepName, string> = {
    download: "meetings.status.download",
    audio: "meetings.status.audio",
    transcript: "meetings.status.transcript",
    summary: "meetings.status.summary",
  };

  /** A job that runs wins over the error of an earlier run: the Rust side sends one of the two, not both. */
  const failed = $derived(row.live === null && row.error !== null);
  const icon = $derived(failed ? "error" : row.kind);

  /** What a row says of how it stands, in one badge: a failure, work in progress, no summary yet, or the action items that are still open. */
  type Badge = { tone: "failed" | "busy" | "plain"; text: string; /** The tooltip. */ hint?: string };

  function percent(progress: number): number {
    return Math.round(Math.min(1, Math.max(0, progress)) * 100);
  }

  /**
   * One badge at most, by priority: failed, processing, not summarized, N actions. A job that runs is "processing" even when an earlier run
   * left its error on the row (see `failed`). The count is of the action items that are open, as the Actions tab counts them, and a
   * recording with none open has no badge.
   */
  const badge = $derived.by((): Badge | null => {
    if (row.live) {
      return {
        tone: "busy",
        text:
          row.live.progress === null
            ? t("meetings.row.badge.processing_unknown")
            : t("meetings.row.badge.processing", { pct: percent(row.live.progress) }),
        hint: row.live.step ? t(STATUS[row.live.step]) : t("meetings.status.waiting"),
      };
    }
    if (row.error) {
      // A cancelled job has nothing to fix: it says so, in the place and the tone of the failed badge.
      return row.error.kind === "Cancelled"
        ? { tone: "failed", text: t("meetings.error.cancelled") }
        : { tone: "failed", text: t("meetings.row.badge.failed"), hint: t("meetings.row.badge.failed_hint") };
    }
    if (!row.hasSummary) return { tone: "plain", text: t("meetings.row.badge.not_summarized") };
    if (row.openTasks > 0) {
      return { tone: "plain", text: row.openTasks === 1 ? t("meetings.row.badge.action") : t("meetings.row.badge.actions", { n: row.openTasks }) };
    }
    return null;
  });

  const duration = $derived(row.duration === null ? "" : formatDuration(row.duration));

  /** The title is cut to one line on screen, so the name of the button holds all of it, with the rest of what the row says. */
  const label = $derived([row.title, duration, badge?.text, failed ? badge?.hint : undefined].filter(Boolean).join(", "));
</script>

<button type="button" class="row" class:selected aria-label={label} aria-current={selected ? "true" : undefined} onclick={() => onselect(row.id)}>
  <span class="icon" class:failed data-icon={icon} aria-hidden="true">
    <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">
      {#if icon === "screen"}
        <rect x="2" y="3" width="12" height="8" rx="1" /><path d="M6 14h4M8 11v3" />
      {:else if icon === "link"}
        <path d="M6.5 9.5l3-3M5 7.5L3.5 9a2.1 2.1 0 003 3L8 10.5M11 8.5L12.5 7a2.1 2.1 0 00-3-3L8 5.5" />
      {:else if icon === "error"}
        <circle cx="8" cy="8" r="6" /><path d="M8 5v3.5M8 11v.01" />
      {:else}
        <path d="M4 2h5l3 3v9H4zM9 2v3h3" />
      {/if}
    </svg>
  </span>
  <span class="text">
    <span class="title" title={row.title}>{row.title}</span>
    <span class="subtitle">
      {#if duration}<span class="duration">{duration}</span>{/if}
      {#if badge}
        <span class="badge {badge.tone}" title={badge.hint}>{#if badge.tone === "busy"}<span class="spinner" aria-hidden="true"></span>{/if}<span>{badge.text}</span></span>
      {/if}
    </span>
  </span>
</button>

<style>
  .row {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    width: 100%;
    padding: 8px 10px;
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 8px;
  }
  .row:hover {
    background: var(--accent-soft);
  }
  .row.selected {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .icon {
    flex: none;
    margin-top: 1px;
    color: var(--muted);
  }
  .icon.failed {
    color: var(--danger);
  }
  .text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .title {
    display: block;
    overflow: hidden;
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* The second line: the duration, then the badge. 16 px high whatever is on it, so that every row is as tall as the next. */
  .subtitle {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    min-height: 16px;
    font-size: 12px;
    line-height: 16px;
    color: var(--muted);
    white-space: nowrap;
  }
  .duration {
    flex: none;
  }
  /* A pill with a line around it and no fill, so that its text keeps the contrast that the text of the list has on the panel and on the
     selected row alike. It gives way (and ends in an ellipsis) when the list is at its narrowest. */
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    padding: 0 6px;
    font-size: 11px;
    line-height: 14px;
    color: var(--muted);
    border: 1px solid var(--line);
    border-radius: 999px;
  }
  /* The words. `overflow: hidden` would cut the marks of a Vietnamese letter (ử, ệ) at the edge of a 14 px line, so the box is a pixel
     taller on each side and the margin takes the pixel back. */
  .badge > span:last-child {
    min-width: 0;
    padding: 1px 0;
    margin: -1px 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .badge.failed {
    color: var(--danger);
    border-color: var(--danger);
  }
  .badge.busy {
    color: var(--warn-fg);
    border-color: var(--warn);
  }
  .spinner {
    flex: none;
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
</style>
