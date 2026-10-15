// Client-side code search over one revision: list the tree recursively, then
// fetch text blobs (size-capped) and grep them. Everything goes through the
// public `tree` endpoint; results stream back through callbacks so the UI can
// show progress and cancel.

import { api } from "../api/client";
import type { TreeEntry } from "../api/types";

export interface SearchFile {
  path: string;
  id: string;
  size: number;
}

export interface SearchMatch {
  line: number;
  text: string;
  /** Column ranges [start, end) of matches within `text`. */
  ranges: [number, number][];
}

export interface SearchHit {
  file: SearchFile;
  matches: SearchMatch[];
  /** True when more matches exist than were kept. */
  more: boolean;
}

export interface SearchProgress {
  phase: "listing" | "grepping" | "done";
  listed: number;
  scanned: number;
  total: number;
  skipped: number;
  hits: number;
}

export interface SearchOptions {
  caseSensitive?: boolean;
  regex?: boolean;
  /** Only scan paths containing this substring (e.g. `src/auth`). */
  pathFilter?: string;
  maxFileBytes?: number;
  maxFiles?: number;
  maxMatchesPerFile?: number;
  maxHits?: number;
}

const DEFAULTS: Required<SearchOptions> = {
  caseSensitive: false,
  regex: false,
  pathFilter: "",
  maxFileBytes: 512 * 1024,
  maxFiles: 2000,
  maxMatchesPerFile: 25,
  maxHits: 300,
};

const BINARY_EXT = /\.(png|jpe?g|gif|webp|ico|bmp|tiff?|pdf|zip|gz|tgz|bz2|xz|7z|rar|jar|war|class|wasm|so|dylib|dll|exe|bin|o|a|lock|woff2?|ttf|otf|eot|mp[34]|mov|avi|mkv|webm|ogg|flac|sqlite|db|parquet|pyc|psd|ai|sketch)$/i;
const CONCURRENCY = 6;

// ---- tree listing cache ---------------------------------------------------------

const listCache = new Map<string, Promise<SearchFile[]>>();

/** Recursively list files of `rev`. Cached per (repo, rev). */
export function listFiles(repo: string, rev: string, onProgress?: (listed: number) => void, signal?: AbortSignal): Promise<SearchFile[]> {
  const key = `${repo}\0${rev}`;
  const cached = listCache.get(key);
  if (cached) return cached;
  const p = (async () => {
    const out: SearchFile[] = [];
    const queue: string[] = [""];
    const walk = async (dir: string) => {
      const t = await api.tree(repo, rev, dir || undefined, signal);
      if (t.kind !== "tree") return;
      for (const e of t.entries as TreeEntry[]) {
        const path = dir ? `${dir}/${e.name}` : e.name;
        if (e.mode === "dir") queue.push(path);
        else if (e.mode !== "submodule") out.push({ path, id: e.id, size: e.size });
      }
      onProgress?.(out.length);
    };
    // Breadth-first with bounded parallelism.
    while (queue.length) {
      const batch = queue.splice(0, CONCURRENCY);
      await Promise.all(batch.map(walk));
    }
    out.sort((a, b) => a.path.localeCompare(b.path));
    return out;
  })().catch((e) => {
    listCache.delete(key);
    throw e;
  });
  listCache.set(key, p);
  return p;
}

// ---- blob cache -----------------------------------------------------------------

const blobCache = new Map<string, string | null>();
const MAX_BLOBS = 600;

async function blobText(repo: string, rev: string, f: SearchFile, signal?: AbortSignal): Promise<string | null> {
  const key = `${repo}\0${f.id}`;
  if (blobCache.has(key)) return blobCache.get(key)!;
  const t = await api.tree(repo, rev, f.path, signal);
  const text = t.kind === "blob" && !t.binary ? (t.text ?? "") : null;
  if (blobCache.size >= MAX_BLOBS) blobCache.delete(blobCache.keys().next().value!);
  blobCache.set(key, text);
  return text;
}

// ---- matcher ----------------------------------------------------------------------

export function buildMatcher(q: string, opts: { caseSensitive?: boolean; regex?: boolean }): ((line: string) => [number, number][]) | null {
  if (!q) return null;
  const flags = opts.caseSensitive ? "g" : "gi";
  let re: RegExp;
  try {
    re = new RegExp(opts.regex ? q : q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), flags);
  } catch {
    return null;
  }
  return (line: string) => {
    const ranges: [number, number][] = [];
    re.lastIndex = 0;
    let m: RegExpExecArray | null;
    while ((m = re.exec(line))) {
      if (m[0].length === 0) {
        re.lastIndex++;
        continue;
      }
      ranges.push([m.index, m.index + m[0].length]);
      if (ranges.length > 50) break;
    }
    return ranges;
  };
}

export interface SearchHandle {
  promise: Promise<SearchHit[]>;
  cancel: () => void;
}

/** Grep `q` across the text files of `rev`. Hits are delivered incrementally through `onHit`. */
export function searchCode(repo: string, rev: string, q: string, options: SearchOptions, onProgress: (p: SearchProgress) => void, onHit: (h: SearchHit) => void): SearchHandle {
  const o = { ...DEFAULTS, ...options };
  const ctl = new AbortController();
  const matcher = buildMatcher(q, o);
  const progress: SearchProgress = { phase: "listing", listed: 0, scanned: 0, total: 0, skipped: 0, hits: 0 };
  const emit = () => onProgress({ ...progress });

  const promise = (async () => {
    const hits: SearchHit[] = [];
    if (!matcher) {
      progress.phase = "done";
      emit();
      return hits;
    }
    const all = await listFiles(repo, rev, (n) => {
      progress.listed = n;
      emit();
    }, ctl.signal);
    const filter = o.pathFilter.toLowerCase();
    const candidates = all.filter((f) => !filter || f.path.toLowerCase().includes(filter));
    const scannable = candidates.filter((f) => f.size <= o.maxFileBytes && !BINARY_EXT.test(f.path)).slice(0, o.maxFiles);
    progress.skipped = candidates.length - scannable.length;
    progress.total = scannable.length;
    progress.phase = "grepping";
    emit();

    let next = 0;
    const worker = async () => {
      while (next < scannable.length && hits.length < o.maxHits) {
        if (ctl.signal.aborted) return;
        const f = scannable[next++];
        let text: string | null;
        try {
          text = await blobText(repo, rev, f, ctl.signal);
        } catch (e) {
          if (ctl.signal.aborted) return;
          text = null;
          void e;
        }
        if (text !== null) {
          const lines = text.split("\n");
          const matches: SearchMatch[] = [];
          let more = false;
          for (let i = 0; i < lines.length; i++) {
            const ranges = matcher(lines[i]);
            if (!ranges.length) continue;
            if (matches.length >= o.maxMatchesPerFile) {
              more = true;
              break;
            }
            matches.push({ line: i + 1, text: lines[i].length > 400 ? lines[i].slice(0, 400) : lines[i], ranges });
          }
          if (matches.length) {
            const hit = { file: f, matches, more };
            hits.push(hit);
            progress.hits = hits.length;
            onHit(hit);
          }
        } else progress.skipped++;
        progress.scanned++;
        emit();
      }
    };
    await Promise.all(Array.from({ length: Math.min(CONCURRENCY, scannable.length) }, worker));
    progress.phase = "done";
    emit();
    return hits;
  })();

  return { promise, cancel: () => ctl.abort() };
}
