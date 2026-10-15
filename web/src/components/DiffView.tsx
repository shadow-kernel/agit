import { Fragment, useMemo, useState, type ReactNode } from "react";
import { parseUnifiedDiff, type FileDiff } from "../lib/diff";
import type { Change } from "../api/types";
import { ChangeKindBadge, ChangePath, DiffStat, EmptyState } from "./ui";

export interface LineAnnotation {
  path: string;
  line: number;
  render: ReactNode;
}

export function DiffView({ patch, changes, annotations = [], onLineClick }: { patch: string; changes?: Change[]; annotations?: LineAnnotation[]; onLineClick?: (path: string, line: number) => void }) {
  const files = useMemo(() => parseUnifiedDiff(patch), [patch]);
  // Renamed files are keyed by their new path in `changes`; the patch may name either side.
  const byPath = useMemo(() => {
    const m = new Map<string, Change>();
    for (const c of changes ?? []) {
      m.set(c.path, c);
      if (c.kind === "renamed" && c.from) m.set(c.from, c);
    }
    return m;
  }, [changes]);
  if (!files.length) {
    // Patch may be empty for binary-only changes: still list the files.
    if (changes?.length) {
      return (
        <div className="box">
          {changes.map((c) => (
            <div key={c.path} className="box-row static">
              <ChangeKindBadge kind={c.kind} />
              <span className="mono"><ChangePath change={c} /></span>
              <span className="muted small">{c.kind === "renamed" && c.similarity === 100 ? "identical content" : "no textual diff"}</span>
            </div>
          ))}
        </div>
      );
    }
    return <EmptyState icon="±" title="No changes">The patch is empty.</EmptyState>;
  }
  return (
    <div>
      {files.map((f) => (
        <FileDiffView key={f.path} file={f} change={byPath.get(f.path) ?? (f.oldPath ? byPath.get(f.oldPath) : undefined)} annotations={annotations.filter((a) => a.path === f.path)} onLineClick={onLineClick} />
      ))}
    </div>
  );
}

function FileDiffView({ file, change, annotations, onLineClick }: { file: FileDiff; change?: Change; annotations: LineAnnotation[]; onLineClick?: (path: string, line: number) => void }) {
  const [open, setOpen] = useState(true);
  const patchRenamed = !!file.oldPath && !!file.newPath && file.oldPath !== file.newPath;
  const effKind: Change["kind"] = change?.kind ?? (file.oldPath === null ? "added" : file.newPath === null ? "deleted" : patchRenamed ? "renamed" : "modified");
  const shown = { path: change?.kind === "renamed" ? change.path : file.path, kind: effKind, from: change?.kind === "renamed" ? change.from : patchRenamed ? file.oldPath : undefined, similarity: change?.kind === "renamed" ? change.similarity : undefined };
  const byLine = useMemo(() => {
    const m = new Map<number, ReactNode[]>();
    for (const a of annotations) m.set(a.line, [...(m.get(a.line) ?? []), a.render]);
    return m;
  }, [annotations]);
  return (
    <div className="box diff-file" id={`diff-${encodeURIComponent(shown.path)}`}>
      <div className="box-head">
        <button className="toggle" onClick={() => setOpen((o) => !o)} aria-expanded={open} aria-label="Toggle file">{open ? "▾" : "▸"}</button>
        <ChangeKindBadge kind={effKind} />
        <span className="path"><ChangePath change={shown} /></span>
        {file.binary ? <span className="muted">binary</span> : file.hunks.length === 0 && effKind === "renamed" ? <span className="muted">renamed without changes</span> : <DiffStat add={file.additions} del={file.deletions} />}
      </div>
      {open && !file.binary ? (
        <table className="diff-table">
          <tbody>
            {file.hunks.map((h, hi) => (
              <Fragment key={hi}>
                <tr className="hunk">
                  <td className="ln" />
                  <td className="ln" />
                  <td className="sign" />
                  <td className="code">{h.header}</td>
                </tr>
                {h.lines.map((l, li) => {
                  const anchorLine = l.newNo ?? l.oldNo;
                  const notes = l.newNo !== null ? byLine.get(l.newNo) : undefined;
                  return (
                    <Fragment key={li}>
                      <tr className={l.type === "add" ? "add" : l.type === "del" ? "del" : l.type === "meta" ? "meta" : ""} onClick={onLineClick && anchorLine ? () => onLineClick(file.path, anchorLine) : undefined} style={onLineClick ? { cursor: "pointer" } : undefined} title={onLineClick ? "Comment on this line" : undefined}>
                        <td className="ln">{l.oldNo ?? ""}</td>
                        <td className="ln">{l.newNo ?? ""}</td>
                        <td className="sign">{l.type === "add" ? "+" : l.type === "del" ? "−" : ""}</td>
                        <td className="code">{l.text || " "}</td>
                      </tr>
                      {notes?.map((n, ni) => (
                        <tr key={`n${ni}`} className="comment-anchor">
                          <td colSpan={4}>{n}</td>
                        </tr>
                      ))}
                    </Fragment>
                  );
                })}
              </Fragment>
            ))}
          </tbody>
        </table>
      ) : null}
    </div>
  );
}
