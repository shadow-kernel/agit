import { useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import type { AgentInfo, Change, ChangeKind, NoteSummary, PullState, SignatureInfo } from "../api/types";
import type { ApiError } from "../api/client";
import { shortId } from "../lib/format";

// ---- Badges -----------------------------------------------------------------

export type Tone = "neutral" | "accent" | "success" | "danger" | "attention" | "done";

export function Badge({ tone = "neutral", children, outline, mono, lg, className = "", title }: { tone?: Tone; children: ReactNode; outline?: boolean; mono?: boolean; lg?: boolean; className?: string; title?: string }) {
  return (
    <span title={title} className={`badge ${tone === "neutral" ? "" : tone} ${outline ? "outline" : ""} ${mono ? "mono" : ""} ${lg ? "lg" : ""} ${className}`.trim()}>
      {children}
    </span>
  );
}

const PALETTE = ["#0969da", "#8250df", "#1a7f37", "#bf8700", "#cf222e", "#0e8a8a", "#d4587c", "#6e5494"];
export function colorFor(id: string) {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return PALETTE[h % PALETTE.length];
}
export function initials(id: string) {
  const parts = id.split(/[-_./\s]+/).filter(Boolean);
  return (parts.length > 1 ? parts[0][0] + parts[parts.length - 1][0] : id.slice(0, 2)).toUpperCase();
}

export function Avatar({ id, lg }: { id: string; lg?: boolean }) {
  return (
    <span className={lg ? "avatar-lg" : "avatar"} style={{ background: colorFor(id) }} aria-hidden>
      {initials(id)}
    </span>
  );
}

export function AgentBadge({ agent, repo, showModel = true }: { agent: AgentInfo | { id: string; model?: string | null } | null | undefined; repo?: string; showModel?: boolean }) {
  if (!agent) return null;
  const inner = (
    <>
      <Avatar id={agent.id} />
      <span>{agent.id}</span>
      {showModel && agent.model ? <span className="model truncate">{agent.model}</span> : null}
    </>
  );
  const title = [agent.id, agent.model, "run_id" in agent && agent.run_id ? `run ${agent.run_id}` : null].filter(Boolean).join(" · ");
  if (repo) {
    return (
      <Link className="agent-badge" title={title} to={`/r/${encodeURIComponent(repo)}/commits?agent=${encodeURIComponent(agent.id)}`}>
        {inner}
      </Link>
    );
  }
  return (
    <span className="agent-badge" title={title}>
      {inner}
    </span>
  );
}

export function TaskChip({ task, repo }: { task: string | null | undefined; repo?: string }) {
  if (!task) return null;
  if (repo) return <Link className="task-chip" to={`/r/${encodeURIComponent(repo)}/commits?task=${encodeURIComponent(task)}`}>{task}</Link>;
  return <span className="task-chip">{task}</span>;
}

export function Sha({ id, repo, n = 8 }: { id: string | null | undefined; repo?: string; n?: number }) {
  if (!id) return <span className="sha">—</span>;
  if (repo) return <Link className="sha" title={id} to={`/r/${encodeURIComponent(repo)}/commit/${id}`}>{shortId(id, n)}</Link>;
  return <span className="sha" title={id}>{shortId(id, n)}</span>;
}

export function Tag({ tag, repo, active, onClick }: { tag: string; repo?: string; active?: boolean; onClick?: () => void }) {
  const ns = tag.split(":")[0];
  const cls = `tag ns-${ns}`;
  if (onClick) return <button type="button" className={cls + (active ? " active" : "")} onClick={onClick} style={{ border: 0, cursor: "pointer" }}>{tag}</button>;
  if (repo) return <Link className={cls} to={`/r/${encodeURIComponent(repo)}/knowledge?tags=${encodeURIComponent(tag)}`}>{tag}</Link>;
  return <span className={cls}>{tag}</span>;
}

export function ChangeKindBadge({ kind }: { kind: ChangeKind }) {
  const letter = { added: "A", modified: "M", deleted: "D", type_changed: "T", renamed: "R" }[kind] ?? "?";
  const label = { added: "Added", modified: "Modified", deleted: "Deleted", type_changed: "Type changed", renamed: "Renamed" }[kind] ?? kind;
  return <span className={`change-kind ${kind}`} title={label}>{letter}</span>;
}

/** File name for a change: `from → path` plus the similarity for renames. */
export function ChangePath({ change, className = "" }: { change: Pick<Change, "path" | "kind" | "from" | "similarity">; className?: string }) {
  if (change.kind === "renamed" && change.from) {
    return (
      <span className={`change-path ${className}`.trim()} title={`${change.from} → ${change.path}`}>
        <span className="from">{change.from}</span>
        <span className="arrow" aria-hidden> → </span>
        <span>{change.path}</span>
        {typeof change.similarity === "number" ? <Badge outline mono className="sim" title={`${change.similarity}% similar to the old file`}>{change.similarity}%</Badge> : null}
      </span>
    );
  }
  return <span className={`change-path ${className}`.trim()}>{change.path}</span>;
}

/** Commit signature state from `CommitDetail.signature`. */
export function SignatureBadge({ sig, lg }: { sig: SignatureInfo | null | undefined; lg?: boolean }) {
  if (!sig || sig.status === "unsigned") return <Badge outline lg={lg} title="This commit carries no Ed25519 signature">Unsigned</Badge>;
  if (sig.status === "invalid") return <Badge tone="danger" lg={lg} title={`Invalid signature: ${sig.reason}`}>Invalid signature</Badge>;
  if (sig.trusted) return <Badge tone="success" lg={lg} title={`Signed by ${sig.signer} with key ${sig.key_id} (registered for the signer)`}>Verified</Badge>;
  return <Badge tone="attention" lg={lg} title={`Valid signature by ${sig.signer}, but key ${sig.key_id} is not registered for that agent`}>Valid, untrusted key</Badge>;
}

/** `draft` / `stale` / `superseded` markers of a note. */
export function NoteBadges({ n }: { n: Pick<NoteSummary, "draft" | "stale" | "superseded_by"> }) {
  return (
    <>
      {n.draft ? <Badge tone="done" title="Machine-generated draft: hidden from listings and context until accepted">draft</Badge> : null}
      {n.stale ? <Badge tone="attention" title="A path this note covers changed since it was last validated">stale</Badge> : null}
      {n.superseded_by ? <Badge tone="attention" outline title={`superseded by ${n.superseded_by}`}>superseded</Badge> : null}
    </>
  );
}

/** BM25 relevance from `GET /knowledge/notes?q=`. */
export function Score({ v }: { v: number | null | undefined }) {
  if (typeof v !== "number") return null;
  return <span className="score" title={`BM25 relevance ${v}`}>score {v.toFixed(2)}</span>;
}

export function CopyButton({ text, label = "Copy", sm = true }: { text: string; label?: string; sm?: boolean }) {
  const [done, setDone] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
    setDone(true);
    setTimeout(() => setDone(false), 1500);
  };
  return <button type="button" className={`btn ${sm ? "sm" : ""}`} onClick={copy} aria-live="polite">{done ? "Copied" : label}</button>;
}

export function PullStateBadge({ state }: { state: PullState }) {
  const tone: Tone = state === "open" ? "success" : state === "merged" ? "done" : "danger";
  const icon = state === "open" ? "⊙" : state === "merged" ? "⑂" : "⊘";
  return (
    <Badge tone={tone} lg>
      <span aria-hidden>{icon}</span> {state[0].toUpperCase() + state.slice(1)}
    </Badge>
  );
}

export function PullStateIcon({ state }: { state: PullState }) {
  const icon = state === "open" ? "⊙" : state === "merged" ? "⑂" : "⊘";
  return <span className={`state-icon state-${state}`} title={state} aria-label={state}>{icon}</span>;
}

export function EventKindBadge({ kind }: { kind: string }) {
  const tone: Tone = kind.startsWith("pull.merge") ? "done" : kind.startsWith("pull.") ? "accent" : kind.startsWith("lease.release") ? "neutral" : kind.startsWith("lease.") ? "attention" : kind === "commit" || kind === "merge" ? "success" : kind === "checkpoint" ? "neutral" : kind.startsWith("note.") ? "done" : kind.startsWith("ref.") ? "accent" : "neutral";
  return <Badge tone={tone} mono>{kind}</Badge>;
}

// ---- States -------------------------------------------------------------------

export function Skeleton({ lines = 3, className = "" }: { lines?: number; className?: string }) {
  const widths = ["80%", "60%", "70%", "40%", "90%", "55%"];
  return (
    <div className={`skeleton-block ${className}`} aria-busy>
      {Array.from({ length: lines }).map((_, i) => (
        <div key={i} className="skeleton skeleton-line" style={{ width: widths[i % widths.length] }} />
      ))}
    </div>
  );
}

export function SkeletonRows({ rows = 6 }: { rows?: number }) {
  return (
    <div aria-busy>
      {Array.from({ length: rows }).map((_, i) => (
        <div key={i} className="box-row static" style={{ gap: 16 }}>
          <div className="skeleton" style={{ width: 16, height: 16 }} />
          <div className="skeleton skeleton-line" style={{ width: `${30 + ((i * 17) % 40)}%`, margin: 0 }} />
          <div className="grow" />
          <div className="skeleton skeleton-line" style={{ width: 80, margin: 0 }} />
        </div>
      ))}
    </div>
  );
}

export function EmptyState({ icon = "○", title, children, action }: { icon?: string; title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty">
      <div className="icon" aria-hidden>{icon}</div>
      <h3>{title}</h3>
      {children ? <p>{children}</p> : null}
      {action ? <div className="mt16">{action}</div> : null}
    </div>
  );
}

export function ErrorState({ error, retry }: { error: ApiError; retry?: () => void }) {
  return (
    <div className="error-box" role="alert">
      <span aria-hidden>⚠</span>
      <div className="grow">
        <div><span className="code">{error.code}</span>{error.status ? <span className="muted small"> · HTTP {error.status}</span> : null}</div>
        <div>{error.message}</div>
      </div>
      {retry ? <button className="btn sm" onClick={retry}>Retry</button> : null}
    </div>
  );
}

export function Banner({ tone = "neutral", icon, title, children, actions }: { tone?: Tone; icon?: string; title: ReactNode; children?: ReactNode; actions?: ReactNode }) {
  return (
    <div className={`banner ${tone === "neutral" ? "" : tone}`}>
      {icon ? <span className="icon" aria-hidden>{icon}</span> : null}
      <div className="grow">
        <div className="title">{title}</div>
        {children ? <div className="small muted">{children}</div> : null}
      </div>
      {actions}
    </div>
  );
}

export function Spinner() {
  return <span className="spinner" aria-label="loading" />;
}

export function DiffStat({ add, del }: { add: number; del: number }) {
  const total = add + del || 1;
  const blocks = 5;
  const a = Math.round((add / total) * blocks);
  const d = Math.min(blocks - a, Math.round((del / total) * blocks));
  return (
    <span className="row" style={{ gap: 6 }}>
      <span className="stat-add">+{add}</span>
      <span className="stat-del">−{del}</span>
      <span className="stat-bar" aria-hidden>
        {Array.from({ length: blocks }).map((_, i) => <i key={i} className={i < a ? "a" : i < a + d ? "d" : ""} />)}
      </span>
    </span>
  );
}
