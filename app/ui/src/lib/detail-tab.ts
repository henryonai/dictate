// The tab of the recording's right column that was chosen last: kept across recordings and launches, like the size of the video.

/** The tabs of the right column, in the order that they are shown. */
export const DETAIL_TABS = ["summary", "actions", "chapters", "ask"] as const;
export type DetailTab = (typeof DETAIL_TABS)[number];
/** Where the choice is kept. */
export const DETAIL_TAB_KEY = "loah.detail.tab";

function isTab(value: string | null): value is DetailTab {
  return DETAIL_TABS.some((tab) => tab === value);
}

/** The tab that was chosen last time; the summary when none was, or when the store can't be read (it may be off). */
export function storedTab(): DetailTab {
  try {
    const stored = localStorage.getItem(DETAIL_TAB_KEY);
    if (isTab(stored)) return stored;
  } catch {
    // A store that is not there or refuses: the pane opens on the summary.
  }
  return "summary";
}

export function storeTab(tab: DetailTab): void {
  try {
    localStorage.setItem(DETAIL_TAB_KEY, tab);
  } catch {
    // A store that is not there or refuses: the choice holds while the pane is shown.
  }
}
