// Client-side blame, kept as the fallback for servers without
// `GET /repos/{repo}/blame/{rev}` (and used by the mock to build its fixture):
// walk the file's history (`GET /commits?path=`), fetch the blob at each commit
// and attribute lines with a Myers line diff, newest commit first.

import { api as facade, ApiError, type Api } from "../api/client";
import type { CommitListItem } from "../api/types";

export interface BlameLine {
  /** 1-based line number in the blamed revision. */
  no: number;
  text: string;
  /** Index into `BlameResult.commits`; -1 when the history was truncated before this line's origin. */
  commit: number;
}

export interface BlameResult {
  lines: BlameLine[];
  /** Newest first, exactly the commits returned by the history walk. */
  commits: CommitListItem[];
  /** True when the history hit the commit limit; lines left over are attributed to the oldest commit seen. */
  truncated: boolean;
  binary: boolean;
}

export interface BlameProgress {
  done: number;
  total: number;
}

const HISTORY_LIMIT = 200;
const CONCURRENCY = 4;

// ---- Myers diff on line arrays ------------------------------------------------

/**
 * Returns, for every index of `a`, the matching index in `b` or -1 when the
 * line was removed/changed. Standard greedy Myers O((N+M)D) with a trace.
 */
export function lineMatches(a: string[], b: string[]): Int32Array {
  const n = a.length, m = b.length;
  const out = new Int32Array(n).fill(-1);
  // Trim common prefix/suffix: they always match.
  let pre = 0;
  while (pre < n && pre < m && a[pre] === b[pre]) {
    out[pre] = pre;
    pre++;
  }
  let suf = 0;
  while (suf < n - pre && suf < m - pre && a[n - 1 - suf] === b[m - 1 - suf]) {
    out[n - 1 - suf] = m - 1 - suf;
    suf++;
  }
  const an = n - pre - suf, bm = m - pre - suf;
  if (an === 0 || bm === 0) return out;
  // Intern lines to ints for cheap comparisons.
  const ids = new Map<string, number>();
  const A = new Int32Array(an), B = new Int32Array(bm);
  const intern = (s: string) => {
    let v = ids.get(s);
    if (v === undefined) ids.set(s, (v = ids.size));
    return v;
  };
  for (let i = 0; i < an; i++) A[i] = intern(a[pre + i]);
  for (let j = 0; j < bm; j++) B[j] = intern(b[pre + j]);

  const max = an + bm;
  const off = max;
  let v = new Int32Array(2 * max + 2);
  const trace: Int32Array[] = [];
  let found = false;
  for (let d = 0; d <= max && !found; d++) {
    trace.push(v);
    const nv = new Int32Array(v);
    for (let k = -d; k <= d; k += 2) {
      let x: number;
      if (k === -d || (k !== d && v[off + k - 1] < v[off + k + 1])) x = v[off + k + 1];
      else x = v[off + k - 1] + 1;
      let y = x - k;
      while (x < an && y < bm && A[x] === B[y]) {
        x++;
        y++;
      }
      nv[off + k] = x;
      if (x >= an && y >= bm) {
        found = true;
        break;
      }
    }
    v = nv;
  }
  // Walk the trace backwards and record diagonal (matching) steps.
  let x = an, y = bm;
  for (let d = trace.length - 1; d >= 0; d--) {
    const vd = trace[d];
    const k = x - y;
    let prevK: number;
    if (k === -d || (k !== d && vd[off + k - 1] < vd[off + k + 1])) prevK = k + 1;
    else prevK = k - 1;
    const prevX = vd[off + prevK];
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) {
      x--;
      y--;
      out[pre + x] = pre + y;
    }
    if (d > 0) {
      x = prevX;
      y = prevY;
    }
  }
  return out;
}

// ---- caches ---------------------------------------------------------------------

const resultCache = new Map<string, Promise<BlameResult>>();
const blobCache = new Map<string, Promise<string | null>>();
const MAX_BLOB_CACHE = 400;

function blobAt(api: Api, repo: string, commitId: string, path: string, signal?: AbortSignal): Promise<string | null> {
  const key = `${repo}\0${commitId}\0${path}`;
  let p = blobCache.get(key);
  if (!p) {
    p = api
      .tree(repo, commitId, path, signal)
      .then((t) => (t.kind === "blob" ? (t.binary ? null : (t.text ?? "")) : null))
      .catch((e) => {
        if (e instanceof ApiError && e.status === 404) return null; // file absent at this commit
        blobCache.delete(key);
        throw e;
      });
    if (blobCache.size >= MAX_BLOB_CACHE) blobCache.delete(blobCache.keys().next().value!);
    blobCache.set(key, p);
  }
  return p;
}

const splitLines = (s: string) => (s === "" ? [] : s.replace(/\n$/, "").split("\n"));

/** Cached per (repo, rev, path). The returned promise is removed from the cache on failure. */
export function blame(repo: string, rev: string, path: string, onProgress?: (p: BlameProgress) => void, signal?: AbortSignal): Promise<BlameResult> {
  const key = `${repo}\0${rev}\0${path}`;
  const cached = resultCache.get(key);
  if (cached) return cached;
  const p = computeBlame(facade, repo, rev, path, onProgress, signal).catch((e) => {
    resultCache.delete(key);
    throw e;
  });
  resultCache.set(key, p);
  return p;
}

/** Uncached; `api` picks the data source (the mock passes itself). */
export async function computeBlame(api: Api, repo: string, rev: string, path: string, onProgress?: (p: BlameProgress) => void, signal?: AbortSignal): Promise<BlameResult> {
  const head = await api.tree(repo, rev, path, signal);
  if (head.kind !== "blob") throw new ApiError(400, "invalid", `${path} is a directory`, false);
  if (head.binary || head.text == null) return { lines: [], commits: [], truncated: false, binary: true };
  const text = head.text;
  const history = await api.commits(repo, { rev, path, limit: HISTORY_LIMIT });
  const total = history.length;
  onProgress?.({ done: 0, total });
  if (total === 0) {
    // No history for this path (should not happen for an existing file): attribute nothing.
    return { lines: splitLines(text).map((t, i) => ({ no: i + 1, text: t, commit: -1 })), commits: [], truncated: false, binary: false };
  }

  // Fetch every version with bounded concurrency; contents[i] is the file at history[i] (null = absent/binary).
  const contents: (string | null)[] = new Array(total).fill(null);
  let next = 0, done = 0;
  const worker = async () => {
    while (next < total) {
      const i = next++;
      if (signal?.aborted) throw new DOMException("aborted", "AbortError");
      contents[i] = i === 0 ? text : await blobAt(api, repo, history[i].id, path, signal);
      done++;
      onProgress?.({ done, total });
    }
  };
  await Promise.all(Array.from({ length: Math.min(CONCURRENCY, total) }, worker));

  const headLines = splitLines(text);
  const attributed = new Int32Array(headLines.length).fill(-1);
  // pos[l] = index of head line l inside contents[i]; -1 once attributed.
  let pos = Int32Array.from(headLines, (_, i) => i);
  let curLines = headLines;
  for (let i = 0; i < total; i++) {
    const older = i + 1 < total ? splitLines(contents[i + 1] ?? "") : [];
    const match = lineMatches(curLines, older);
    const nextPos = new Int32Array(pos.length).fill(-1);
    let remaining = 0;
    for (let l = 0; l < pos.length; l++) {
      const p = pos[l];
      if (p < 0) continue;
      const q = match[p];
      if (q < 0) attributed[l] = i;
      else {
        nextPos[l] = q;
        remaining++;
      }
    }
    pos = nextPos;
    curLines = older;
    if (remaining === 0) break;
  }
  const truncated = total >= HISTORY_LIMIT;
  for (let l = 0; l < attributed.length; l++) if (attributed[l] < 0) attributed[l] = total - 1; // lines older than the walk
  return {
    lines: headLines.map((t, i) => ({ no: i + 1, text: t, commit: attributed[i] })),
    commits: history,
    truncated,
    binary: false,
  };
}
