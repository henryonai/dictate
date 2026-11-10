<script lang="ts">
  import type { MeetingRow, StepName } from "../lib/api";
  import { formatDuration } from "../lib/days";
  import { currentLanguage, t } from "../lib/i18n";

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

  function time(created: number): string {
    return new Date(created * 1000).toLocaleTimeString(currentLanguage(), { hour: "numeric", minute: "2-digit" });
  }

  function actions(n: number): string {
    return n === 1 ? t("meetings.row.action") : t("meetings.row.actions", { n });
  }

  const line = $derived.by((): { text: string; tone: "done" | "failed" | "busy" } => {
    if (row.live) {
      const status = row.live.step ? t(STATUS[row.live.step]) : t("meetings.status.waiting");
      const percent = row.live.progress === null ? "" : ` · ${Math.round(Math.min(1, Math.max(0, row.live.progress)) * 100)}%`;
      return { text: status + percent, tone: "busy" };
    }
    if (row.error) {
      return {
        text: row.error.kind === "Cancelled" ? t("meetings.error.cancelled") : t("meetings.row.failed"),
        tone: "failed",
      };
    }
    if (!row.hasSummary) return { text: t("meetings.row.not_summarized"), tone: "done" };
    const parts = [
      row.created ? time(row.created) : "",
      row.duration === null ? "" : formatDuration(row.duration),
      row.openTasks > 0 ? actions(row.openTasks) : "",
    ];
    return { text: parts.filter(Boolean).join(" · "), tone: "done" };
  });
</script>

<button type="button" class="row" class:selected aria-current={selected ? "true" : undefined} onclick={() => onselect(row.id)}>
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
    <span class="title">{row.title}</span>
    <span class="subtitle {line.tone}" class:failed={line.tone === "failed"}>{#if line.tone === "busy"}<span class="spinner" aria-hidden="true"></span>{/if}<span>{line.text}</span></span>
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
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    overflow-wrap: anywhere;
    font-weight: 500;
  }
  .subtitle {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }
  .subtitle > span:last-child {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .subtitle.failed {
    color: var(--danger);
  }
  .subtitle.busy {
    color: var(--warn-fg);
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
