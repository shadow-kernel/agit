// Parser for unified diffs (the `patch` string returned by the API).

export interface DiffLine {
  type: "context" | "add" | "del" | "meta";
  text: string;
  oldNo: number | null;
  newNo: number | null;
}

export interface DiffHunk {
  header: string;
  oldStart: number;
  newStart: number;
  lines: DiffLine[];
}

export interface FileDiff {
  oldPath: string | null;
  newPath: string | null;
  path: string;
  hunks: DiffHunk[];
  additions: number;
  deletions: number;
  binary: boolean;
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/;

function stripPrefix(p: string): string | null {
  const t = p.trim();
  if (t === "/dev/null") return null;
  return t.replace(/^[ab]\//, "");
}

export function parseUnifiedDiff(patch: string): FileDiff[] {
  const files: FileDiff[] = [];
  if (!patch) return files;
  const lines = patch.split("\n");
  let cur: FileDiff | null = null;
  let hunk: DiffHunk | null = null;
  let oldNo = 0, newNo = 0;

  const startFile = (oldPath: string | null, newPath: string | null) => {
    cur = { oldPath, newPath, path: newPath ?? oldPath ?? "?", hunks: [], additions: 0, deletions: 0, binary: false };
    files.push(cur);
    hunk = null;
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (line.startsWith("diff --git ")) {
      const m = /^diff --git a\/(.+?) b\/(.+)$/.exec(line);
      startFile(m ? m[1] : null, m ? m[2] : null);
      continue;
    }
    if (line.startsWith("--- ") && (lines[i + 1] ?? "").startsWith("+++ ")) {
      const o = stripPrefix(line.slice(4));
      const n = stripPrefix(lines[i + 1].slice(4));
      if (!cur || cur.hunks.length > 0) startFile(o, n);
      else {
        cur.oldPath = o;
        cur.newPath = n;
        cur.path = n ?? o ?? cur.path;
      }
      i++;
      continue;
    }
    if (line.startsWith("Binary files") || line.startsWith("GIT binary patch")) {
      if (cur) cur.binary = true;
      continue;
    }
    const hm = HUNK.exec(line);
    if (hm) {
      if (!cur) startFile(null, null);
      oldNo = parseInt(hm[1], 10);
      newNo = parseInt(hm[3], 10);
      hunk = { header: line, oldStart: oldNo, newStart: newNo, lines: [] };
      cur!.hunks.push(hunk);
      continue;
    }
    if (!hunk || !cur) continue;
    if (line === "" && i === lines.length - 1) continue;
    const c = line[0];
    if (c === "+") {
      hunk.lines.push({ type: "add", text: line.slice(1), oldNo: null, newNo: newNo++ });
      cur.additions++;
    } else if (c === "-") {
      hunk.lines.push({ type: "del", text: line.slice(1), oldNo: oldNo++, newNo: null });
      cur.deletions++;
    } else if (c === "\\") {
      hunk.lines.push({ type: "meta", text: line, oldNo: null, newNo: null });
    } else {
      hunk.lines.push({ type: "context", text: line.slice(1), oldNo: oldNo++, newNo: newNo++ });
    }
  }
  return files;
}
