import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, type StreamStatus } from "../api/client";
import type { Event } from "../api/types";
import { fullDate, shortId, timeAgo } from "../lib/format";
import { useDismiss } from "../lib/useDismiss";
import { useNow } from "../lib/useAsync";
import { Avatar, EventKindBadge } from "./ui";

const KEEP = 20;

interface ReadState {
  upTo: number;
  seqs: number[];
}
const keyFor = (repo: string) => `agit.notifications.${repo}`;
function loadRead(repo: string): ReadState {
  try {
    const raw = localStorage.getItem(keyFor(repo));
    if (raw) {
      const p = JSON.parse(raw) as Partial<ReadState>;
      return { upTo: Number(p.upTo) || 0, seqs: Array.isArray(p.seqs) ? p.seqs.map(Number) : [] };
    }
  } catch {
    /* ignore */
  }
  return { upTo: 0, seqs: [] };
}
function saveRead(repo: string, s: ReadState) {
  try {
    localStorage.setItem(keyFor(repo), JSON.stringify({ upTo: s.upTo, seqs: s.seqs.slice(-200) }));
  } catch {
    /* ignore */
  }
}

/** Human summary + link target for an event (shared shape with the activity feed). */
export function describeEvent(e: Event, base: string): { text: string; to: string } {
  const d = e.data ?? {};
  const str = (k: string) => (typeof d[k] === "string" ? (d[k] as string) : undefined);
  const num = (k: string) => (typeof d[k] === "number" ? (d[k] as number) : undefined);
  if ((e.kind === "commit" || e.kind === "checkpoint" || e.kind === "merge") && str("id")) {
    const branch = str("branch");
    const summary = str("summary") ?? (str("message") ? str("message")!.split("\n")[0] : undefined);
    const who = str("author");
    return { text: `${shortId(str("id"), 8)}${branch ? ` on ${branch}` : ""}${summary ? ` · ${summary}` : ""}${str("intent") ? ` → ${str("intent")}` : ""}${!e.actor && who ? ` (${who})` : ""}`, to: `${base}/commit/${str("id")}` };
  }
  if (e.kind.startsWith("pull.")) {
    const n = num("number");
    const extra = str("title") ?? str("decision") ?? (e.kind === "pull.comment" ? `${str("kind") ?? "comment"}${str("path") ? ` on ${str("path")}` : ""}` : str("merged_commit") ? `merged as ${shortId(str("merged_commit"), 8)}` : "");
    return { text: `${n !== undefined ? `#${n}` : "pull request"}${extra ? ` · ${extra}` : ""}`, to: n !== undefined ? `${base}/pulls/${n}` : `${base}/pulls` };
  }
  if (e.kind.startsWith("lease.")) {
    const paths = Array.isArray(d.paths) ? (d.paths as string[]) : [];
    return { text: `${paths.join(", ") || str("id") || ""}${str("mode") ? ` (${str("mode")})` : ""}`, to: `${base}/leases` };
  }
  if (e.kind === "ref.update") {
    const name = str("ref") ?? str("name") ?? "";
    const branch = name.replace(/^refs\/heads\//, "");
    return { text: `${branch} → ${str("new") ? shortId(str("new"), 8) : "deleted"}`, to: str("new") ? `${base}/commit/${str("new")}` : `${base}/commits` };
  }
  if (e.kind.startsWith("note.")) return { text: str("title") ?? str("id") ?? "", to: `${base}/knowledge/${encodeURIComponent(str("id") ?? "")}` };
  if (e.kind.startsWith("policy")) return { text: "repository policy updated", to: `${base}/settings` };
  if (e.kind === "knowledge.rules") return { text: `knowledge rules updated${num("count") !== undefined ? ` (${num("count")} rules)` : ""}`, to: `${base}/settings/rules` };
  if (e.kind.startsWith("webhook")) return { text: str("url") ?? str("id") ?? "", to: `${base}/settings/webhooks` };
  if (e.kind === "repo.create" || e.kind === "repo.init") return { text: str("name") ?? "repository created", to: base };
  const json = JSON.stringify(d);
  return { text: json.length > 80 ? json.slice(0, 77) + "…" : json, to: `${base}/agents` };
}

export function NotificationBell({ repo }: { repo: string }) {
  const [events, setEvents] = useState<Event[]>([]);
  const [read, setRead] = useState<ReadState>(() => loadRead(repo));
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<StreamStatus>("connecting");
  const ref = useRef<HTMLDivElement>(null);
  const navigate = useNavigate();
  const now = useNow(30_000);
  const base = `/r/${encodeURIComponent(repo)}`;
  const close = useCallback(() => setOpen(false), []);
  useDismiss(ref, open, close);

  useEffect(() => {
    setRead(loadRead(repo));
    setEvents([]);
    let alive = true;
    let unsub = () => {};
    const push = (e: Event) => setEvents((xs) => (xs.some((x) => x.seq === e.seq) ? xs : [e, ...xs].sort((a, b) => b.seq - a.seq).slice(0, KEEP)));
    api
      .events(repo, 0, KEEP)
      .then((evs) => {
        if (!alive) return;
        setEvents([...evs].sort((a, b) => b.seq - a.seq).slice(0, KEEP));
        unsub = api.streamEvents(repo, evs.length ? Math.max(...evs.map((e) => e.seq)) : 0, push, setStatus);
      })
      .catch(() => {
        if (!alive) return;
        setStatus("error");
        unsub = api.streamEvents(repo, 0, push, setStatus);
      });
    return () => {
      alive = false;
      unsub();
    };
  }, [repo]);

  const readSet = useMemo(() => new Set(read.seqs), [read.seqs]);
  const isUnread = (e: Event) => e.seq > read.upTo && !readSet.has(e.seq);
  const unread = events.filter(isUnread).length;

  const markAll = () => {
    const next = { upTo: Math.max(read.upTo, ...events.map((e) => e.seq)), seqs: [] };
    setRead(next);
    saveRead(repo, next);
  };
  const markOne = (e: Event) => {
    if (!isUnread(e)) return;
    const next = { upTo: read.upTo, seqs: [...read.seqs, e.seq] };
    setRead(next);
    saveRead(repo, next);
  };

  return (
    <div className="popover-wrap" ref={ref}>
      <button className={`btn sm bell ${open ? "active" : ""}`} onClick={() => setOpen((o) => !o)} aria-haspopup="dialog" aria-expanded={open} aria-label={`Notifications${unread ? `, ${unread} unread` : ""}`} title="Notifications">
        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden fill="currentColor">
          <path d="M8 16a2 2 0 0 0 1.985-1.75c.017-.137-.097-.25-.235-.25h-3.5c-.138 0-.252.113-.235.25A2 2 0 0 0 8 16ZM3 5a5 5 0 0 1 10 0v2.947c0 .05.015.098.042.139l1.703 2.555A1.519 1.519 0 0 1 13.482 13H2.518a1.516 1.516 0 0 1-1.263-2.36l1.703-2.554A.255.255 0 0 0 3 7.947Zm5-3.5A3.5 3.5 0 0 0 4.5 5v2.947c0 .346-.102.683-.294.97l-1.703 2.556a.017.017 0 0 0-.003.01l.001.006c0 .002.002.004.004.006l.006.004.007.001h10.964l.007-.001.006-.004.004-.006.001-.007a.017.017 0 0 0-.003-.01l-1.703-2.554a1.745 1.745 0 0 1-.294-.97V5A3.5 3.5 0 0 0 8 1.5Z" />
        </svg>
        {unread ? <span className="bell-count" aria-hidden>{unread > 9 ? "9+" : unread}</span> : null}
      </button>
      {open ? (
        <div className="popover notif" role="dialog" aria-label="Notifications">
          <div className="row between">
            <h3>
              Notifications <span className="muted" style={{ fontWeight: 400 }}>· {repo}</span>
            </h3>
            <span className={`live-dot ${status === "error" ? "off" : status === "sse" ? "" : "warn"}`} title={status === "sse" ? "live (SSE)" : status === "polling" ? "live (polling)" : status === "error" ? "disconnected" : "connecting…"} />
          </div>
          <div className="notif-list">
            {events.length === 0 ? (
              <div className="muted small" style={{ padding: "12px 0" }}>{status === "error" ? "Event stream unavailable." : "No events yet."}</div>
            ) : (
              events.map((e) => {
                const d = describeEvent(e, base);
                const un = isUnread(e);
                return (
                  <button
                    key={e.seq}
                    type="button"
                    className={`notif-item ${un ? "unread" : ""}`}
                    onClick={() => {
                      markOne(e);
                      setOpen(false);
                      navigate(d.to);
                    }}
                    title={fullDate(e.time_ms)}
                  >
                    <span className="dot" aria-hidden />
                    <div className="grow" style={{ minWidth: 0 }}>
                      <div className="row wrap" style={{ gap: 6 }}>
                        <EventKindBadge kind={e.kind} />
                        {e.actor ? <span className="row small" style={{ gap: 4 }}><Avatar id={e.actor} /> {e.actor}</span> : null}
                        <span className="muted small" style={{ marginLeft: "auto" }}>{timeAgo(e.time_ms, now)}</span>
                      </div>
                      {d.text ? <div className="small truncate" style={{ marginTop: 2 }}>{d.text}</div> : null}
                    </div>
                  </button>
                );
              })
            )}
          </div>
          <div className="row between">
            <Link to={`${base}/agents`} className="small" onClick={() => setOpen(false)}>All activity</Link>
            <button className="btn sm" onClick={markAll} disabled={!unread}>Mark all as read</button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
