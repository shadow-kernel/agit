import { useMemo, useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api } from "../api/client";
import { CommitList } from "../components/CommitList";
import { DiffView } from "../components/DiffView";
import { useRepo } from "../components/RepoShell";
import { Banner, ChangeKindBadge, DiffStat, EmptyState, ErrorState, Skeleton, SkeletonRows } from "../components/ui";
import { pluralize, shortId, summaryLine } from "../lib/format";
import { useAsync } from "../lib/useAsync";

const SEP = "...";

export function parseRange(splat: string): { base: string; head: string } | null {
  const s = decodeURIComponent(splat || "");
  const i = s.indexOf(SEP);
  if (i < 0) return null;
  const base = s.slice(0, i), head = s.slice(i + SEP.length);
  return base && head ? { base, head } : null;
}

export default function Compare() {
  const { name, defaultBranch } = useRepo();
  const { "*": splat = "" } = useParams();
  const navigate = useNavigate();
  const range = parseRange(splat);
  const basePath = `/r/${encodeURIComponent(name)}`;
  const branches = useAsync(() => api.branches(name), [name], { silent: true });
  const tags = useAsync(() => api.tags(name), [name], { silent: true });

  const initial = range ?? { base: defaultBranch, head: "" };
  const [form, setForm] = useState(initial);
  const refOptions = useMemo(() => {
    const names = [...(branches.data ?? []).map((b) => b.name), ...(tags.data ?? []).map((t) => t.name)];
    for (const v of [form.base, form.head]) if (v && !names.includes(v)) names.push(v);
    return names;
  }, [branches.data, tags.data, form.base, form.head]);

  // Pick a sensible head once branches arrive and nothing was chosen.
  const suggestedHead = !form.head ? (branches.data ?? []).find((b) => b.name !== form.base && (b.ahead ?? 0) > 0)?.name ?? (branches.data ?? []).find((b) => b.name !== form.base)?.name ?? "" : form.head;

  const go = (e: FormEvent) => {
    e.preventDefault();
    const head = form.head || suggestedHead;
    if (!form.base || !head) return;
    navigate(`${basePath}/compare/${form.base}${SEP}${head}`);
  };

  return (
    <div className="stack">
      <div className="page-head" style={{ marginBottom: 0 }}>
        <div>
          <h1>Compare changes</h1>
          <p className="muted">Compare any two branches, tags or commits (<code>GET /diff?from=&amp;to=</code>), then open a pull request from the result.</p>
        </div>
      </div>
      <form className="box compare-picker" onSubmit={go}>
        <div className="box-row static wrap">
          <RefPicker label="base" value={form.base} options={refOptions} onChange={(v) => setForm({ ...form, base: v })} />
          <span className="muted" aria-hidden>←</span>
          <RefPicker label="compare" value={form.head || suggestedHead} options={refOptions} onChange={(v) => setForm({ ...form, head: v })} />
          <button type="button" className="btn sm ghost" onClick={() => setForm({ base: form.head || suggestedHead, head: form.base })} title="Swap base and compare">⇄ swap</button>
          <div className="grow" />
          <button type="submit" className="btn sm accent" disabled={!form.base || !(form.head || suggestedHead)}>Compare</button>
        </div>
      </form>
      {range ? <CompareResult key={`${range.base}${SEP}${range.head}`} base={range.base} head={range.head} isBranch={(branches.data ?? []).some((b) => b.name === range.head)} /> : (
        <div className="box"><EmptyState icon="⇄" title="Choose two revisions">Pick a base and a branch to compare, or open <code>{basePath}/compare/main...feature</code> directly.</EmptyState></div>
      )}
    </div>
  );
}

function RefPicker({ label, value, options, onChange }: { label: string; value: string; options: string[]; onChange: (v: string) => void }) {
  const listId = `refs-${label}`;
  return (
    <label className="row ref-picker" style={{ gap: 6 }}>
      <span className="small muted">{label}:</span>
      <input className="input sm mono" list={listId} value={value} onChange={(e) => onChange(e.target.value)} placeholder="branch, tag or id" spellCheck={false} aria-label={`${label} revision`} style={{ width: 200 }} />
      <datalist id={listId}>{options.map((o) => <option key={o} value={o} />)}</datalist>
    </label>
  );
}

function CompareResult({ base, head, isBranch }: { base: string; head: string; isBranch: boolean }) {
  const { name, defaultBranch } = useRepo();
  const basePath = `/r/${encodeURIComponent(name)}`;
  const diff = useAsync(() => api.diff(name, base, head), [name, base, head]);
  const headCommits = useAsync(() => api.commits(name, { rev: head, limit: 250 }), [name, head], { silent: true });
  const baseCommits = useAsync(() => api.commits(name, { rev: base, limit: 250 }), [name, base], { silent: true });
  const ahead = useMemo(() => {
    if (!headCommits.data || !baseCommits.data) return undefined;
    const inBase = new Set(baseCommits.data.map((c) => c.id));
    return headCommits.data.filter((c) => !inBase.has(c.id));
  }, [headCommits.data, baseCommits.data]);
  const totals = useMemo(() => (diff.data?.stats ?? []).reduce((a, s) => ({ add: a.add + s.added, del: a.del + s.removed }), { add: 0, del: 0 }), [diff.data]);
  const statByPath = useMemo(() => new Map((diff.data?.stats ?? []).map((s) => [s.path, s])), [diff.data]);

  if (diff.error) return <ErrorState error={diff.error} retry={diff.reload} />;
  if (!diff.data) return <div className="box"><Skeleton lines={6} /></div>;
  const d = diff.data;
  const prTitle = ahead?.length === 1 ? summaryLine(ahead[0].message) : "";
  const prIntent = ahead?.length === 1 ? ahead[0].intent ?? "" : "";
  const prTask = ahead?.length === 1 ? ahead[0].task ?? "" : "";
  const prHref = `${basePath}/pulls/new?source=${encodeURIComponent(`refs/heads/${head}`)}&target=${encodeURIComponent(`refs/heads/${base}`)}${prTitle ? `&title=${encodeURIComponent(prTitle)}` : ""}${prIntent ? `&intent=${encodeURIComponent(prIntent)}` : ""}${prTask ? `&task=${encodeURIComponent(prTask)}` : ""}`;

  if (base === head || (d.changes.length === 0 && (ahead?.length ?? 0) === 0)) {
    return (
      <div className="box">
        <EmptyState icon="≡" title={base === head ? "Nothing to compare" : "These revisions are identical"}>
          <span className="ref-pill">{shortLabel(base)}</span> and <span className="ref-pill">{shortLabel(head)}</span> point at the same tree.
        </EmptyState>
      </div>
    );
  }

  return (
    <div className="stack">
      <Banner
        tone={d.changes.length ? "success" : "neutral"}
        icon="⇄"
        title={<><span className="ref-pill">{shortLabel(base)}</span> ← <span className="ref-pill">{shortLabel(head)}</span> · {ahead ? pluralize(ahead.length, "commit") : "commits…"} · {pluralize(d.changes.length, "file")} changed · <span className="stat-add">+{totals.add}</span> <span className="stat-del">−{totals.del}</span></>}
        actions={isBranch ? <Link className="btn primary" to={prHref}>Create pull request</Link> : <span className="small muted" title="Pull requests need a branch as the compare side">compare side is not a branch</span>}
      >
        {isBranch ? <>Opens <code>{basePath}/pulls/new</code> with source <code>refs/heads/{head}</code> and target <code>refs/heads/{base}</code>{base !== defaultBranch ? "" : " (the default branch)"}.</> : null}
      </Banner>

      <div className="box">
        <div className="box-head"><h3>Commits</h3><span className="muted">on {shortLabel(head)} and not on {shortLabel(base)}{headCommits.data && headCommits.data.length >= 250 ? " (latest 250 inspected)" : ""}</span></div>
        {!ahead ? <SkeletonRows rows={3} /> : ahead.length === 0 ? <EmptyState icon="◷" title="No commits ahead">The trees differ but {shortLabel(head)} has no commits that are missing from {shortLabel(base)}; {shortLabel(base)} may be ahead instead.</EmptyState> : null}
      </div>
      {ahead?.length ? <CommitList repo={name} commits={ahead} groupByDay={false} /> : null}

      <div className="box">
        <div className="box-head"><h3>{pluralize(d.changes.length, "file")} changed</h3><span className="stat-add">+{totals.add}</span><span className="stat-del">−{totals.del}</span></div>
        <div className="file-list">
          {d.changes.map((ch) => {
            const s = statByPath.get(ch.path);
            return (
              <div key={ch.path} className="box-row">
                <ChangeKindBadge kind={ch.kind} />
                <a href={`#diff-${encodeURIComponent(ch.path)}`} className="grow truncate">{ch.path}</a>
                {s ? <DiffStat add={s.added} del={s.removed} /> : null}
                {ch.kind !== "deleted" ? <Link className="btn sm ghost" to={`${basePath}/blob/${encodeURIComponent(head)}/${ch.path}`}>view</Link> : null}
              </div>
            );
          })}
        </div>
      </div>
      <DiffView patch={d.patch} changes={d.changes} />
    </div>
  );
}

const shortLabel = (r: string) => (/^[0-9a-f]{12,64}$/.test(r) ? shortId(r, 8) : r);
