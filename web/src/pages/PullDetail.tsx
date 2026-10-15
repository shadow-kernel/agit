import { useMemo, useState, type FormEvent } from "react";
import { Link, NavLink, Route, Routes, useParams } from "react-router-dom";
import { api } from "../api/client";
import type { CommentKind, MergeStrategy, PullComment, PullDetail as PullDetailT, Review, ReviewDecision } from "../api/types";
import { CommitList } from "../components/CommitList";
import { DiffView, type LineAnnotation } from "../components/DiffView";
import { Markdown } from "../components/Markdown";
import { useRepo } from "../components/RepoShell";
import { toast } from "../components/Toasts";
import { AgentBadge, Avatar, Badge, Banner, ChangeKindBadge, DiffStat, EmptyState, ErrorState, PullStateBadge, Sha, Skeleton, TaskChip, type Tone } from "../components/ui";
import { fullDate, pluralize, refShort, timeAgo } from "../lib/format";
import { useAsync, useMutation } from "../lib/useAsync";

export default function PullDetail() {
  const { name } = useRepo();
  const { n = "0" } = useParams();
  const num = Number(n);
  const detail = useAsync(() => api.pull(name, num), [name, num]);
  const base = `/r/${encodeURIComponent(name)}/pulls/${num}`;

  if (detail.error) return <ErrorState error={detail.error} retry={detail.reload} />;
  if (!detail.data) return <div className="stack"><Skeleton lines={2} /><div className="box"><Skeleton lines={6} /></div></div>;
  const d = detail.data;
  const { pr } = d;
  const unresolved = pr.comments.filter((c) => !c.resolved).length;

  return (
    <div className="stack">
      <div className="pr-title">
        <div className="row between wrap" style={{ alignItems: "flex-start" }}>
          <h1>
            {pr.title} <span className="num">#{pr.number}</span>
          </h1>
          <div className="row">
            {pr.state === "open" ? <Link className="btn sm" to={`/r/${encodeURIComponent(name)}/pulls/new?source=${encodeURIComponent(pr.source)}`}>Duplicate</Link> : null}
          </div>
        </div>
        <div className="pr-sub">
          <PullStateBadge state={pr.state} />
          <AgentBadge agent={pr.author} repo={name} />
          <span>
            wants to merge {pluralize(d.commits.length, "commit")} into <span className="ref-pill">{refShort(pr.target)}</span> from <span className="ref-pill">{refShort(pr.source)}</span>
          </span>
          <TaskChip task={pr.task} repo={name} />
          <span title={fullDate(pr.created_ms)}>opened {timeAgo(pr.created_ms)}</span>
        </div>
        {pr.intent ? <div className="intent mt8">{pr.intent}</div> : null}
      </div>

      <nav className="tabs" aria-label="Pull request sections">
        <NavLink to={base} end className={({ isActive }) => (isActive ? "active" : "")}>Conversation {unresolved ? <span className="count">{unresolved}</span> : null}</NavLink>
        <NavLink to={`${base}/commits`} className={({ isActive }) => (isActive ? "active" : "")}>Commits <span className="count">{d.commits.length}</span></NavLink>
        <NavLink to={`${base}/files`} className={({ isActive }) => (isActive ? "active" : "")}>Files changed <span className="count">{d.changes.length}</span></NavLink>
        <NavLink to={`${base}/checks`} className={({ isActive }) => (isActive ? "active" : "")}>Checks <span className="count">{pr.checks.length}</span></NavLink>
      </nav>

      <Routes>
        <Route index element={<Conversation d={d} setData={detail.setData} reload={detail.reload} />} />
        <Route path="commits" element={d.commits.length ? <CommitList repo={name} commits={d.commits} groupByDay={false} /> : <div className="box"><EmptyState icon="◷" title="No commits">The source branch is not ahead of the base.</EmptyState></div>} />
        <Route path="files" element={<FilesChanged d={d} setData={detail.setData} />} />
        <Route path="checks" element={<Checks d={d} />} />
      </Routes>
    </div>
  );
}

// ---- Conversation ---------------------------------------------------------------

function Conversation({ d, setData, reload }: { d: PullDetailT; setData: (u: PullDetailT) => void; reload: () => void }) {
  const { name, reloadCounts } = useRepo();
  const { pr } = d;
  const timeline = useMemo(() => {
    const items: { time: number; node: React.ReactNode }[] = [];
    pr.reviews.forEach((r, i) => items.push({ time: r.time_ms, node: <ReviewCard key={`r${i}`} r={r} /> }));
    pr.comments.forEach((c) => items.push({ time: c.time_ms, node: <CommentCard key={`c${c.id}`} c={c} pr={pr.number} onResolved={(cm) => setData({ ...d, pr: { ...pr, comments: pr.comments.map((x) => (x.id === cm.id ? cm : x)) } })} /> }));
    return items.sort((a, b) => a.time - b.time);
  }, [pr, d, setData]);

  return (
    <div className="two-col">
      <div className="stack">
        <div className="comment-card">
          <div className="card-head">
            <Avatar id={pr.author.id} />
            <strong>{pr.author.id}</strong>
            <span className="time" title={fullDate(pr.created_ms)}>opened this pull request {timeAgo(pr.created_ms)}</span>
          </div>
          <div className={`card-body ${pr.body ? "" : "empty-body"}`}>{pr.body ? <Markdown source={pr.body} compact /> : "No description provided."}</div>
        </div>
        {timeline.length ? <div className="timeline">{timeline.map((t) => t.node)}</div> : null}
        <MergeBox d={d} onChanged={() => { reload(); reloadCounts(); }} />
        {pr.state === "open" ? <ReviewForm d={d} setData={setData} /> : null}
      </div>
      <aside className="sidebar">
        <section>
          <h3>Reviewers</h3>
          {pr.reviews.length === 0 ? <span className="muted small">No reviews yet</span> : null}
          <div className="stack tight">
            {latestByReviewer(pr.reviews).map((r) => (
              <div key={r.reviewer} className="row small">
                <Avatar id={r.reviewer} /> <span className="grow truncate">{r.reviewer}</span> <DecisionBadge d={r.decision} />
              </div>
            ))}
          </div>
        </section>
        <section>
          <h3>Checks</h3>
          {pr.checks.length === 0 ? <span className="muted small">No checks reported</span> : null}
          <div className="stack tight">
            {pr.checks.map((c) => (
              <div key={c.name} className="row small">
                <span className={`status-dot ${c.status}`} /> <span className="grow truncate">{c.name}</span> <span className="muted">{c.status}</span>
              </div>
            ))}
          </div>
        </section>
        <section>
          <h3>Details</h3>
          <dl>
            <dt>Source</dt><dd><Sha id={pr.source_id} repo={name} /></dd>
            <dt>Target</dt><dd><Sha id={pr.target_id} repo={name} /></dd>
            <dt>Base</dt><dd><Sha id={pr.base_id} repo={name} /></dd>
            {pr.merged_commit ? <><dt>Merged</dt><dd><Sha id={pr.merged_commit} repo={name} /></dd></> : null}
            <dt>Updated</dt><dd title={fullDate(pr.updated_ms)}>{timeAgo(pr.updated_ms)}</dd>
            <dt>Model</dt><dd>{pr.author.model ?? <span className="subtle">—</span>}</dd>
          </dl>
        </section>
        <section>
          <h3>Comments</h3>
          <div className="small muted">
            {pluralize(pr.comments.length, "comment")}, {pr.comments.filter((c) => !c.resolved).length} unresolved
            {pr.comments.some((c) => c.kind === "blocker" && !c.resolved) ? <div style={{ color: "var(--danger)" }}>Open blockers prevent merging.</div> : null}
          </div>
        </section>
      </aside>
    </div>
  );
}

function latestByReviewer(reviews: Review[]) {
  const m = new Map<string, Review>();
  for (const r of reviews) m.set(r.reviewer, r);
  return [...m.values()];
}

function DecisionBadge({ d }: { d: ReviewDecision }) {
  const tone: Tone = d === "approve" ? "success" : d === "request_changes" ? "danger" : "neutral";
  const label = d === "approve" ? "approved" : d === "request_changes" ? "changes requested" : "commented";
  return <Badge tone={tone}>{label}</Badge>;
}

function ReviewCard({ r }: { r: Review }) {
  const icon = r.decision === "approve" ? "✓" : r.decision === "request_changes" ? "±" : "💬";
  return (
    <div className="review-card">
      <span className={`icon ${r.decision}`} aria-hidden>{icon}</span>
      <div className="grow">
        <div className="row wrap small" style={{ gap: 6 }}>
          <strong>{r.reviewer}</strong>
          <DecisionBadge d={r.decision} />
          <span className="muted" title={fullDate(r.time_ms)}>{timeAgo(r.time_ms)}</span>
        </div>
        {r.body ? <div className="comment-card mt8"><div className="card-body"><Markdown source={r.body} compact /></div></div> : null}
      </div>
    </div>
  );
}

const KIND_TONE: Record<CommentKind, Tone> = { blocker: "danger", suggestion: "accent", question: "attention", note: "neutral" };

function CommentCard({ c, pr, onResolved, compact }: { c: PullComment; pr: number; onResolved: (c: PullComment) => void; compact?: boolean }) {
  const { name } = useRepo();
  const resolve = useMutation(() => api.resolvePullComment(name, pr, c.id));
  const doResolve = async () => {
    const r = await resolve.run();
    if (r !== undefined) onResolved({ ...c, resolved: true });
  };
  return (
    <div className={`comment-card ${c.kind === "blocker" && !c.resolved ? "blocker" : ""} ${c.resolved ? "resolved" : ""}`}>
      <div className="card-head">
        <Avatar id={c.author} />
        <strong>{c.author}</strong>
        <Badge tone={KIND_TONE[c.kind] ?? "neutral"}>{c.kind}</Badge>
        {c.path && !compact ? (
          <Link className="path-ref" to={`/r/${encodeURIComponent(name)}/pulls/${pr}/files#diff-${encodeURIComponent(c.path)}`}>
            {c.path}{c.line ? `:${c.line}` : ""}
          </Link>
        ) : null}
        <span className="time" title={fullDate(c.time_ms)}>{timeAgo(c.time_ms)}</span>
        <div className="grow" />
        {c.resolved ? <Badge tone="success" outline>resolved</Badge> : <button className="btn sm" onClick={doResolve} disabled={resolve.busy}>Resolve</button>}
      </div>
      <div className="card-body"><Markdown source={c.body} compact /></div>
    </div>
  );
}

function MergeBox({ d, onChanged }: { d: PullDetailT; onChanged: () => void }) {
  const { name } = useRepo();
  const { pr } = d;
  const [strategy, setStrategy] = useState<MergeStrategy>("merge");
  const [message, setMessage] = useState("");
  const merge = useMutation(() => api.mergePull(name, pr.number, { strategy, message: message || undefined }));
  const close = useMutation(() => api.closePull(name, pr.number));
  const blockers = pr.comments.filter((c) => c.kind === "blocker" && !c.resolved);
  const failing = pr.checks.filter((c) => c.status === "failure");
  const pending = pr.checks.filter((c) => c.status === "pending");
  const changesRequested = latestByReviewer(pr.reviews).filter((r) => r.decision === "request_changes");
  const approvals = latestByReviewer(pr.reviews).filter((r) => r.decision === "approve");

  if (pr.state === "merged") return <Banner tone="done" icon="⑂" title={`Merged into ${refShort(pr.target)}`}>{pr.merged_commit ? <>Merge commit <Sha id={pr.merged_commit} repo={name} /> · {timeAgo(pr.updated_ms)}</> : null}</Banner>;
  if (pr.state === "closed") return <Banner tone="danger" icon="⊘" title="Closed without merging">{timeAgo(pr.updated_ms)}</Banner>;

  let tone: Tone = "success", title = "This branch has no conflicts with the base branch", icon = "✓", sub: React.ReactNode = "Merging can be performed automatically.";
  if (d.conflicts.length) {
    tone = "danger"; icon = "✕"; title = "This branch has conflicts that must be resolved";
    sub = <>Conflicting files: {d.conflicts.map((c) => <code key={c} style={{ marginRight: 6 }}>{c}</code>)}</>;
  } else if (blockers.length) {
    tone = "danger"; icon = "!"; title = `${pluralize(blockers.length, "open blocker comment")} must be resolved before merging`;
    sub = "Blockers are structured review comments; resolve them in the conversation.";
  } else if (!d.mergeable) {
    tone = "attention"; icon = "!"; title = "Not mergeable"; sub = "The server reports this pull request as not mergeable.";
  } else if (failing.length) {
    tone = "danger"; icon = "✕"; title = `${pluralize(failing.length, "check")} failing`; sub = failing.map((c) => c.name).join(", ");
  } else if (changesRequested.length) {
    tone = "attention"; icon = "±"; title = "Changes requested"; sub = `${changesRequested.map((r) => r.reviewer).join(", ")} requested changes.`;
  } else if (pending.length) {
    tone = "attention"; icon = "●"; title = `${pluralize(pending.length, "check")} pending`; sub = "You can still merge; the server applies policy.";
  }
  const colors: Record<Tone, string> = { success: "var(--success)", danger: "var(--danger)", attention: "var(--attention)", done: "var(--done)", accent: "var(--accent)", neutral: "var(--fg-muted)" };

  const doMerge = async () => {
    const r = await merge.run();
    if (r) {
      toast(`Merged #${pr.number} as ${r.merged_commit.slice(0, 8)}`, "success");
      onChanged();
    }
  };
  const doClose = async () => {
    if (!confirm(`Close pull request #${pr.number} without merging?`)) return;
    const r = await close.run();
    if (r) {
      toast(`Closed #${pr.number}`, "info");
      onChanged();
    }
  };

  return (
    <div className="merge-box">
      <div className="merge-head">
        <span className="icon" style={{ background: colors[tone] }} aria-hidden>{icon}</span>
        <div className="grow">
          <div style={{ fontWeight: 600 }}>{title}</div>
          <div className="small muted">{sub}</div>
          {approvals.length ? <div className="small mt8" style={{ color: "var(--success)" }}>✓ Approved by {approvals.map((r) => r.reviewer).join(", ")}</div> : null}
        </div>
      </div>
      <div className="merge-actions">
        <select className="select sm" value={strategy} onChange={(e) => setStrategy(e.target.value as MergeStrategy)} aria-label="Merge strategy">
          <option value="merge">Create a merge commit</option>
          <option value="ff">Fast-forward</option>
        </select>
        {strategy === "merge" ? <input className="input sm" style={{ width: 260 }} placeholder="Merge message (optional)" value={message} onChange={(e) => setMessage(e.target.value)} /> : null}
        <button className="btn primary sm" onClick={doMerge} disabled={merge.busy || d.conflicts.length > 0 || blockers.length > 0}>{merge.busy ? "Merging…" : "Merge pull request"}</button>
        <div className="grow" />
        <button className="btn sm danger" onClick={doClose} disabled={close.busy}>Close pull request</button>
      </div>
    </div>
  );
}

function ReviewForm({ d, setData }: { d: PullDetailT; setData: (u: PullDetailT) => void }) {
  const { name } = useRepo();
  const [decision, setDecision] = useState<ReviewDecision>("comment");
  const [body, setBody] = useState("");
  const review = useMutation(() => api.reviewPull(name, d.pr.number, { decision, body }));
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (decision !== "approve" && !body.trim()) return;
    const pr = await review.run();
    if (pr) {
      setData({ ...d, pr });
      setBody("");
      toast(decision === "approve" ? "Approved" : decision === "request_changes" ? "Changes requested" : "Comment posted", "success");
    }
  };
  return (
    <form className="review-form" onSubmit={submit}>
      <h3>Submit a review</h3>
      <textarea className="textarea" placeholder="Leave a review (markdown)…" value={body} onChange={(e) => setBody(e.target.value)} />
      <div className="row between wrap">
        <div className="choices" role="radiogroup">
          <label><input type="radio" name="decision" checked={decision === "comment"} onChange={() => setDecision("comment")} /> Comment</label>
          <label><input type="radio" name="decision" checked={decision === "approve"} onChange={() => setDecision("approve")} /> Approve</label>
          <label><input type="radio" name="decision" checked={decision === "request_changes"} onChange={() => setDecision("request_changes")} /> Request changes</label>
        </div>
        <button type="submit" className={`btn ${decision === "approve" ? "primary" : decision === "request_changes" ? "danger" : ""}`} disabled={review.busy || (decision !== "approve" && !body.trim())}>
          {decision === "approve" ? "Approve" : decision === "request_changes" ? "Request changes" : "Comment"}
        </button>
      </div>
    </form>
  );
}

// ---- Files changed --------------------------------------------------------------

function FilesChanged({ d, setData }: { d: PullDetailT; setData: (u: PullDetailT) => void }) {
  const { name } = useRepo();
  const diff = useAsync(() => api.pullDiff(name, d.pr.number), [name, d.pr.number]);
  const [draft, setDraft] = useState<{ path: string; line: number } | null>(null);
  const [kind, setKind] = useState<CommentKind>("note");
  const [body, setBody] = useState("");
  const add = useMutation(() => api.addPullComment(name, d.pr.number, { path: draft!.path, line: draft!.line, kind, body }));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!draft || !body.trim()) return;
    const c = await add.run();
    if (c) {
      setData({ ...d, pr: { ...d.pr, comments: [...d.pr.comments, c] } });
      setDraft(null);
      setBody("");
      toast("Comment added", "success");
    }
  };

  const annotations: LineAnnotation[] = [
    ...d.pr.comments.filter((c) => c.path && c.line).map((c) => ({
      path: c.path!,
      line: c.line!,
      render: <CommentCard key={c.id} c={c} pr={d.pr.number} compact onResolved={(cm) => setData({ ...d, pr: { ...d.pr, comments: d.pr.comments.map((x) => (x.id === cm.id ? cm : x)) } })} />,
    })),
    ...(draft
      ? [{
          path: draft.path,
          line: draft.line,
          render: (
            <form key="draft" className="review-form" onSubmit={submit} onClick={(e) => e.stopPropagation()}>
              <div className="row between">
                <strong className="small">Comment on {draft.path}:{draft.line}</strong>
                <select className="select sm" value={kind} onChange={(e) => setKind(e.target.value as CommentKind)}>
                  <option value="note">note</option>
                  <option value="question">question</option>
                  <option value="suggestion">suggestion</option>
                  <option value="blocker">blocker</option>
                </select>
              </div>
              <textarea autoFocus className="textarea" style={{ minHeight: 64 }} value={body} onChange={(e) => setBody(e.target.value)} placeholder="Markdown…" />
              <div className="row" style={{ justifyContent: "flex-end" }}>
                <button type="button" className="btn sm" onClick={() => setDraft(null)}>Cancel</button>
                <button type="submit" className="btn sm accent" disabled={add.busy || !body.trim()}>Add comment</button>
              </div>
            </form>
          ),
        }]
      : []),
  ];

  if (diff.error) return <ErrorState error={diff.error} retry={diff.reload} />;
  if (!diff.data) return <div className="box"><Skeleton lines={8} /></div>;
  const stats = diff.data.stats ?? {};
  return (
    <div className="stack">
      <div className="box">
        <div className="box-head">
          <h3>{pluralize(diff.data.changes.length, "file")} changed</h3>
          <DiffStat add={Number(stats.insertions ?? 0)} del={Number(stats.deletions ?? 0)} />
          <div className="grow" />
          <span className="muted small">Click a line to comment</span>
        </div>
        <div className="file-list">
          {diff.data.changes.map((ch) => (
            <div key={ch.path} className="box-row">
              <ChangeKindBadge kind={ch.kind} />
              <a href={`#diff-${encodeURIComponent(ch.path)}`} className="grow truncate">{ch.path}</a>
              {d.conflicts.includes(ch.path) ? <Badge tone="danger">conflict</Badge> : null}
              {d.pr.comments.filter((c) => c.path === ch.path).length ? <span className="muted small">💬 {d.pr.comments.filter((c) => c.path === ch.path).length}</span> : null}
            </div>
          ))}
        </div>
      </div>
      <DiffView patch={diff.data.patch} changes={diff.data.changes} annotations={annotations} onLineClick={d.pr.state === "open" ? (path, line) => setDraft({ path, line }) : undefined} />
    </div>
  );
}

// ---- Checks -----------------------------------------------------------------------

function Checks({ d }: { d: PullDetailT }) {
  const { pr } = d;
  if (!pr.checks.length) return <div className="box"><EmptyState icon="●" title="No checks reported">CI reports checks with <code>POST /pulls/{pr.number}/checks</code>.</EmptyState></div>;
  return (
    <div className="box">
      {pr.checks.map((c) => (
        <div key={c.name} className="check-row">
          <span className={`status-dot ${c.status}`} />
          <strong className="grow">{c.name}</strong>
          <span className="muted">{c.status}</span>
          <span className="muted small" title={fullDate(c.time_ms)}>{timeAgo(c.time_ms)}</span>
          {c.url ? <a className="btn sm" href={c.url} target="_blank" rel="noreferrer">Details ↗</a> : null}
        </div>
      ))}
    </div>
  );
}
