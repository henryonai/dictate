// The Markdown of a summary or an answer, as the window shows it: the few marks that a model writes (headings, lists, bold) and the
// time stamps that seek the player. It is HTML for `{@html}` and nothing else of the window is: so it is the one place that has to
// make sure that no text of a model or of a recording can be a tag.
//
// The text is escaped first, whole, so that none of it can be a tag or leave an attribute. What the rules add after that is only
// `<strong>` and the seek links, and a link has no address but "#": its seconds are a number that is computed here.

const ESCAPED: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (mark) => ESCAPED[mark]);
}

/** `[mm:ss]` or `[h:mm:ss]` that no `(` follows: with one it is the start of a Markdown link, which is none of ours. */
const STAMP = /\[((?:\d+:)?\d{1,2}:\d{2})\](?!\()/g;

/** The seconds of `mm:ss` or `h:mm:ss`. */
function seconds(stamp: string): number {
  return stamp.split(":").reduce((total, part) => total * 60 + Number(part), 0);
}

/** One line (already escaped) with its leading mark of a heading or a list made visible. */
function mark(line: string): string {
  const heading = /^(#{1,3}) +(.*)$/.exec(line);
  if (heading) {
    const bold = `<strong>${heading[2]}</strong>`;
    // `#` is the title: a bold line. `##` and `###` begin a part: after a line break.
    return heading[1] === "#" ? bold : `\n${bold}`;
  }
  if (/^- \[ \] /.test(line)) return `☐ ${line.slice(6)}`;
  if (/^- \[[xX]\] /.test(line)) return `☑ ${line.slice(6)}`;
  if (line.startsWith("- ")) return `•  ${line.slice(2)}`;
  const nested = /^(?: {2,}|\t+)- (.*)$/.exec(line);
  if (nested) return `      ◦  ${nested[1]}`;
  return line;
}

/**
 * The HTML of a text of Markdown that has only what a summary has: `# x` as a bold line, `## x` and `### x` as a bold line after a
 * line break, `- [ ] ` and `- [x] ` as ☐ and ☑, `- ` and `  - ` as bullets, `**x**` as bold, and each time stamp as a link that
 * seeks (`<a href="#" data-seek="<seconds>">`). Line breaks are kept as they are: the box that shows it keeps its white space.
 */
export function renderInline(markdown: string): string {
  return escapeHtml(markdown.replace(/\r\n?/g, "\n"))
    .split("\n")
    .map(mark)
    .join("\n")
    .replace(/\*\*(?=\S)(.+?)(?<=\S)\*\*/g, "<strong>$1</strong>")
    .replace(STAMP, (stamp, time: string) => `<a href="#" data-seek="${seconds(time)}">${stamp}</a>`)
    .replace(/\n{3,}/g, "\n\n")
    .replace(/^\n+/, "");
}
