// Lazy-loaded highlight.js (common languages only).
type HLJS = typeof import("highlight.js/lib/common").default;
let loader: Promise<HLJS> | null = null;

export function loadHljs(): Promise<HLJS> {
  if (!loader) loader = import("highlight.js/lib/common").then((m) => m.default);
  return loader;
}

const MAX_HIGHLIGHT_BYTES = 400_000;

/** Returns an array of HTML strings, one per line. */
export async function highlightLines(code: string, lang?: string): Promise<string[]> {
  if (code.length > MAX_HIGHLIGHT_BYTES) return code.split("\n").map(escapeHtml);
  const hljs = await loadHljs();
  let html: string;
  try {
    html = lang && hljs.getLanguage(lang) ? hljs.highlight(code, { language: lang, ignoreIllegals: true }).value : hljs.highlightAuto(code).value;
  } catch {
    return code.split("\n").map(escapeHtml);
  }
  return splitHighlightedLines(html);
}

export function escapeHtml(s: string) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** Split hljs output on newlines while keeping span nesting valid per line. */
function splitHighlightedLines(html: string): string[] {
  const out: string[] = [];
  const open: string[] = [];
  let cur = "";
  const re = /<span class="([^"]*)">|<\/span>|\n/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html))) {
    cur += html.slice(last, m.index);
    last = re.lastIndex;
    if (m[0] === "\n") {
      out.push(cur + "</span>".repeat(open.length));
      cur = open.map((c) => `<span class="${c}">`).join("");
    } else if (m[0] === "</span>") {
      open.pop();
      cur += "</span>";
    } else {
      open.push(m[1]);
      cur += m[0];
    }
  }
  cur += html.slice(last);
  out.push(cur + "</span>".repeat(open.length));
  return out;
}
