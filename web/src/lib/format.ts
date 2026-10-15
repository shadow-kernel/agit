export const shortId = (id: string | null | undefined, n = 8) => (id ? id.slice(0, n) : "");

export function timeAgo(ms: number, now = Date.now()): string {
  const diff = Math.max(0, now - ms);
  const s = Math.floor(diff / 1000);
  if (s < 45) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} hour${h === 1 ? "" : "s"} ago`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d} day${d === 1 ? "" : "s"} ago`;
  const mo = Math.floor(d / 30);
  if (mo < 12) return `${mo} month${mo === 1 ? "" : "s"} ago`;
  const y = Math.floor(mo / 12);
  return `${y} year${y === 1 ? "" : "s"} ago`;
}

export function countdown(expiresMs: number, now = Date.now()): string {
  const diff = expiresMs - now;
  if (diff <= 0) return "expired";
  const s = Math.floor(diff / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h}h ${String(m).padStart(2, "0")}m`;
  return `${m}:${String(sec).padStart(2, "0")}`;
}

const dateFmt = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
const dayFmt = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric", year: "numeric" });
export const fullDate = (ms: number) => dateFmt.format(new Date(ms));
export const dayLabel = (ms: number) => dayFmt.format(new Date(ms));
export const dayKey = (ms: number) => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
};

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export const summaryLine = (message: string) => message.split("\n")[0];
export const bodyOf = (message: string) => message.split("\n").slice(1).join("\n").trim();

export const refShort = (ref: string) => ref.replace(/^refs\/heads\//, "").replace(/^refs\/tags\//, "");

export function pluralize(n: number, one: string, many = one + "s") {
  return `${n} ${n === 1 ? one : many}`;
}

export const langOf = (path: string): string | undefined => {
  const ext = path.split(".").pop()?.toLowerCase();
  const map: Record<string, string> = {
    ts: "typescript", tsx: "typescript", js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
    rs: "rust", py: "python", go: "go", java: "java", kt: "kotlin", rb: "ruby", php: "php", cs: "csharp", c: "c", h: "c", cpp: "cpp", hpp: "cpp", cc: "cpp",
    json: "json", yml: "yaml", yaml: "yaml", toml: "ini", ini: "ini", md: "markdown", html: "xml", xml: "xml", svg: "xml", css: "css", scss: "scss", less: "less",
    sh: "bash", bash: "bash", zsh: "bash", sql: "sql", swift: "swift", dockerfile: "dockerfile", lock: "yaml", graphql: "graphql", diff: "diff", patch: "diff",
  };
  if (!ext) return undefined;
  if (path.toLowerCase().endsWith("dockerfile")) return "dockerfile";
  return map[ext];
};
