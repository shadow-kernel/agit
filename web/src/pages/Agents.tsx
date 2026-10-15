import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { api, type StreamStatus } from "../api/client";
import type { Event } from "../api/types";
import { useRepo } from "../components/RepoShell";
import { AgentBadge, Badge, EmptyState, ErrorState, EventKindBadge, Sha, SkeletonRows } from "../components/ui";
import { fullDate, timeAgo } from "../lib/format";
import { useAsync, useNow } from "../lib/useAsync";

const MAX_EVENTS = 200;

export default function Agents() {
  const { name } = useRepo();
  const agents = useAsync(() => api.agents(name), [name]);
  const now = useNow(10_000);
  const [events, setEvents] = useState<Event[]>([]);
  const [status, setStatus] = useState<StreamStatus>("connecting");
  const [paused, setPaused] = useState(false);
  const [kindFilter, setKindFilter] = useState("");
  const [actorFilter, setActorFilter] = useState("");
  const buffer = useRef<Event[]>([]);
  const pausedRef = useRef(false);
  pausedRef.current = paused;

  useEffect(() => {
    let alive = true;
    let unsub = () => {};
    setEvents([]);
    // Initial backlog, then stream from the last seq.
    api
      .events(name, 0, 1000)
      .then((evs) => {
        if (!alive) return;
        const last = evs.slice(-MAX_EVENTS);
        setEvents(last.reverse());
        const since = evs.length ? evs[evs.length - 1].seq : 0;
        unsub = api.streamEvents(
          name,
          since,
          (e) => {
            if (pausedRef.current) {
              buffer.current.push(e);
              return;
            }
            setEvents((xs) => (xs.some((x) => x.seq === e.seq) ? xs : [e, ...xs].slice(0, MAX_EVENTS)));
          },
          setStatus,
        );
      })
      .catch(() => {
        if (!alive) return;
        setStatus("error");
        unsub = api.streamEvents(name, 0, (e) => setEvents((xs) => [e, ...xs].slice(0, MAX_EVENTS)), setStatus);
      });
    return () => {
      alive = false;
      unsub();
    };
  }, [name]);

  useEffect(() => {
    if (!paused && buffer.current.length) {
      const b = buffer.current.splice(0);
      setEvents((xs) => [...b.reverse(), ...xs].slice(0, MAX_EVENTS));
    }
  }, [paused]);

  const shown = events.filter((e) => (!kindFilter || e.kind.startsWith(kindFilter)) && (!actorFilter || e.actor === actorFilter));
  const kinds = [...new Set(events.map((e) => e.kind.split(".")[0]))].sort();
  const liveLabel = status === "sse" ? "live (SSE)" : status === "polling" ? "live (polling)" : status === "error" ? "disconnected" : "connecting…";

  return (
    <div className="stack">
      <div className="box">
        <div className="box-head"><h3>Agents</h3><span className="muted">derived from commits and events</span></div>
        {agents.error ? <div className="box-body"><ErrorState error={agents.error} retry={agents.reload} /></div> : null}
        {agents.loading && !agents.data ? (
          <SkeletonRows rows={4} />
        ) : agents.data?.length === 0 ? (
          <EmptyState icon="◉" title="No agent activity yet">Commits carrying <code>agent.id</code> show up here.</EmptyState>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Agent</th>
                <th className="num">Commits</th>
                <th className="num">Open PRs</th>
                <th className="num">Leases</th>
                <th>Last seen</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {agents.data?.map((a) => {
                const active = now - a.last_seen_ms < 15 * 60_000;
                return (
                  <tr key={a.id}>
                    <td><span className="row"><AgentBadge agent={{ id: a.id }} repo={name} />{active ? <span className="live-dot" title="active in the last 15 minutes" /> : null}</span></td>
                    <td className="num">{a.commits}</td>
                    <td className="num">{a.open_pulls ? <Link to={`/r/${encodeURIComponent(name)}/pulls`}>{a.open_pulls}</Link> : 0}</td>
                    <td className="num">{a.leases ? <Link to={`/r/${encodeURIComponent(name)}/leases`}>{a.leases}</Link> : 0}</td>
                    <td title={fullDate(a.last_seen_ms)}>{a.last_seen_ms ? timeAgo(a.last_seen_ms, now) : "—"}</td>
                    <td className="right-align">
                      <button className="btn sm ghost" onClick={() => setActorFilter(actorFilter === a.id ? "" : a.id)}>{actorFilter === a.id ? "Show all events" : "Filter events"}</button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>

      <div className="box">
        <div className="box-head">
          <span className={`live-dot ${status === "error" ? "off" : status === "polling" || status === "connecting" ? "warn" : ""}`} />
          <h3>Live activity</h3>
          <span className="muted">{liveLabel} · last {shown.length} of {events.length}</span>
          <div className="grow" />
          <select className="select sm" value={kindFilter} onChange={(e) => setKindFilter(e.target.value)} aria-label="Filter by kind">
            <option value="">All kinds</option>
            {kinds.map((k) => <option key={k} value={k}>{k}.*</option>)}
          </select>
          {actorFilter ? <Badge tone="accent">{actorFilter} <button className="btn ghost" style={{ height: 16, padding: 0 }} onClick={() => setActorFilter("")} aria-label="clear">×</button></Badge> : null}
          <button className={`btn sm ${paused ? "active" : ""}`} onClick={() => setPaused((p) => !p)}>{paused ? `Resume${buffer.current.length ? ` (${buffer.current.length})` : ""}` : "Pause"}</button>
        </div>
        <div className="feed">
          {shown.length === 0 ? <EmptyState icon="◌" title={events.length ? "No events match the filter" : "Waiting for events"} /> : shown.map((e) => <EventRow key={e.seq} e={e} repo={name} now={now} />)}
        </div>
      </div>
    </div>
  );
}

function EventRow({ e, repo, now }: { e: Event; repo: string; now: number }) {
  const base = `/r/${encodeURIComponent(repo)}`;
  const d = e.data ?? {};
  const str = (k: string) => (typeof d[k] === "string" ? (d[k] as string) : undefined);
  const num = (k: string) => (typeof d[k] === "number" ? (d[k] as number) : undefined);
  let summary: React.ReactNode;
  if ((e.kind === "commit" || e.kind === "checkpoint" || e.kind === "merge") && str("id")) {
    summary = <><Sha id={str("id")} repo={repo} /> {str("branch") ? <span className="muted">on {str("branch")}</span> : null} {str("intent") ? <span className="intent truncate" style={{ maxWidth: 360 }}>{str("intent")}</span> : null}</>;
  } else if (e.kind.startsWith("pull.") && num("number") !== undefined) {
    summary = <><Link to={`${base}/pulls/${num("number")}`}>#{num("number")}</Link> {str("title") ? <span className="truncate" style={{ maxWidth: 360 }}>{str("title")}</span> : null} {str("decision") ? <Badge>{str("decision")}</Badge> : null} {str("merged_commit") ? <Sha id={str("merged_commit")} repo={repo} /> : null}</>;
  } else if (e.kind.startsWith("lease.")) {
    const paths = Array.isArray(d.paths) ? (d.paths as string[]) : [];
    summary = <>{paths.map((p) => <code key={p}>{p}</code>)} {str("mode") ? <span className="muted">{str("mode")}</span> : null} {num("expires_ms") ? <span className="muted">expires {timeAgo(num("expires_ms")!, now).replace(" ago", "")}</span> : null} <Link className="small" to={`${base}/leases`}>leases</Link></>;
  } else if (e.kind === "ref.update") {
    summary = <><code>{str("name")}</code> <span className="muted">→</span> <Sha id={str("new")} repo={repo} /></>;
  } else if (e.kind === "note.create") {
    summary = <Link to={`${base}/knowledge/${encodeURIComponent(str("id") ?? "")}`}>{str("title") ?? str("id")}</Link>;
  } else {
    summary = <span className="data" title={JSON.stringify(d)}>{JSON.stringify(d)}</span>;
  }
  return (
    <div className="event-row">
      <span className="seq" title="sequence number">#{e.seq}</span>
      <div className="body">
        <EventKindBadge kind={e.kind} />
        {e.actor ? <AgentBadge agent={{ id: e.actor }} repo={repo} /> : null}
        {summary}
        {e.workspace ? <span className="subtle small">ws:{e.workspace}</span> : null}
      </div>
      <span className="time" title={fullDate(e.time_ms)}>{timeAgo(e.time_ms, now)}</span>
    </div>
  );
}
