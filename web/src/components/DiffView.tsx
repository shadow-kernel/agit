import { Fragment, useMemo, useState, type ReactNode } from "react";
import { parseUnifiedDiff, type FileDiff } from "../lib/diff";
import type { Change } from "../api/types";
import { ChangeKindBadge, DiffStat, EmptyState } from "./ui";

export interface LineAnnotation {
  path: string;
  line: number;
  render: ReactNode;
}

export function DiffView({ patch, changes, annotations = [], onLineClick }: { patch: string; changes?: Change[]; annotations?: LineAnnotation[]; onLineClick?: (path: string, line: number) => void }) {
  const files = useMemo(() => parseUnifiedDiff(patch), [patch]);
  const kinds = useMemo(() => new Map((changes ?? []).map((c) => [c.path, c.kind])), [changes]);
  if (!files.length) {
    // Patch may be empty for binary-only changes: still list the files.
    if (changes?.length) {
      return (
        <div className="box">
          {changes.map((c) => (
            <div key={c.path} className="box-row static">
              <ChangeKindBadge kind={c.kind} />
              <span className="mono">{c.path}</span>
              <span className="muted small">no textual diff</span>
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
        <FileDiffView key={f.path} file={f} kind={kinds.get(f.path)} annotations={annotations.filter((a) => a.path === f.path)} onLineClick={onLineClick} />
      ))}
    </div>
  );
}

function FileDiffView({ file, kind, annotations, onLineClick }: { file: FileDiff; kind?: Change["kind"]; annotations: LineAnnotation[]; onLineClick?: (path: string, line: number) => void }) {
  const [open, setOpen] = useState(true);
  const effKind = kind ?? (file.oldPath === null ? "added" : file.newPath === null ? "deleted" : "modified");
  const byLine = useMemo(() => {
    const m = new Map<number, ReactNode[]>();
    for (const a of annotations) m.set(a.line, [...(m.get(a.line) ?? []), a.render]);
    return m;
  }, [annotations]);
  const renamed = file.oldPath && file.newPath && file.oldPath !== file.newPath;
  return (
    <div className="box diff-file" id={`diff-${encodeURIComponent(file.path)}`}>
      <div className="box-head">
        <button className="toggle" onClick={() => setOpen((o) => !o)} aria-expanded={open} aria-label="Toggle file">{open ? "▾" : "▸"}</button>
        <ChangeKindBadge kind={effKind} />
        <span className="path">{renamed ? `${file.oldPath} → ${file.newPath}` : file.path}</span>
        {file.binary ? <span className="muted">binary</span> : <DiffStat add={file.additions} del={file.deletions} />}
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
