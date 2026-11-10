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

/** Whether the character is white space (or there is none): a `**` that touches it can't open or close a pair. */
function blank(char: string): boolean {
  return char === "" || /\s/.test(char);
}

/**
 * Each `**` that opens (a non-space follows it) and the first `**` after it that closes (a non-space comes before it, and at least one
 * character lies between them) make a `<strong>`. One pass over one line: a closing `**` is looked for once, and when there is none no
 * later opener has one either, so a line of unpaired `**` is not searched again for each of them.
 */
function bold(line: string): string {
  let out = "";
  let done = 0;
  let open = line.indexOf("**");
  while (open !== -1) {
    if (blank(line.charAt(open + 2))) {
      open = line.indexOf("**", open + 2);
      continue;
    }
    let close = line.indexOf("**", open + 3);
    while (close !== -1 && blank(line.charAt(close - 1))) close = line.indexOf("**", close + 1);
    if (close === -1) break;
    out += `${line.slice(done, open)}<strong>${line.slice(open + 2, close)}</strong>`;
    done = close + 2;
    open = line.indexOf("**", done);
  }
  return out + line.slice(done);
}

/** One line (already escaped) with its leading mark of a heading or a list made visible, and its bold text. */
function mark(line: string): string {
  const heading = /^(#{1,3}) +(.*)$/.exec(line);
  if (heading) {
    const title = `<strong>${bold(heading[2])}</strong>`;
    // `#` is the title: a bold line. `##` and `###` begin a part: after a line break.
    return heading[1] === "#" ? title : `\n${title}`;
  }
  if (/^- \[ \] /.test(line)) return `☐ ${bold(line.slice(6))}`;
  if (/^- \[[xX]\] /.test(line)) return `☑ ${bold(line.slice(6))}`;
  if (line.startsWith("- ")) return `•  ${bold(line.slice(2))}`;
  const nested = /^(?: {2,}|\t+)- (.*)$/.exec(line);
  if (nested) return `      ◦  ${bold(nested[1])}`;
  return bold(line);
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
    .replace(STAMP, (stamp, time: string) => `<a href="#" data-seek="${seconds(time)}">${stamp}</a>`)
    .replace(/\n{3,}/g, "\n\n")
    .replace(/^\n+/, "");
}
