// Tiny fake agit server built on node:http. It records every request (method, path,
// query, headers, parsed body) so tests can assert what the SDK sent, and answers a
// handful of routes with fixtures shaped like docs/API.md.

import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";

export interface SeenRequest {
  method: string;
  path: string;
  query: URLSearchParams;
  headers: Record<string, string | string[] | undefined>;
  body: unknown;
  rawBody: Buffer;
}

export interface FakeServer {
  baseUrl: string;
  requests: SeenRequest[];
  /** Number of SSE connections opened so far. */
  sseConnections: number;
  close(): Promise<void>;
}

const ID_A = "a".repeat(64);
const ID_B = "b".repeat(64);

export const fixtures = {
  health: { status: "ok", version: "0.1.0" },
  repos: [{ name: "demo", default_branch: "main", head: ID_A, created_ms: 1, branches: 1, tags: 0, objects: 3 }],
  pull: {
    number: 1,
    title: "Rotate session tokens",
    body: "",
    intent: "Close the fixation window",
    task: "SEC-12",
    source: "refs/heads/ws/agent1",
    target: "refs/heads/main",
    source_id: ID_B,
    target_id: ID_A,
    base_id: ID_A,
    author: { id: "agent1", model: null },
    state: "open",
    created_ms: 2,
    updated_ms: 2,
    merged_commit: null,
    reviews: [],
    comments: [],
    checks: [],
  },
  note: {
    id: "n_1",
    title: "Session store is eventually consistent",
    body: "Reads can lag by 200 ms; retry once.",
    tags: ["area:auth", "kind:gotcha"],
    paths: ["src/auth/**"],
    commits: [ID_A],
    pulls: [1],
    author: "agent1",
    confidence: 0.9,
    created_ms: 3,
    updated_ms: 3,
    superseded_by: null,
  },
  policy: {
    protected_branches: ["main"],
    require_pull_request: true,
    require_approvals: 1,
    require_intent: true,
    require_agent: false,
    require_checks: false,
    block_on_blockers: true,
    allowed_merge_agents: [],
    allowed_push_agents: [],
    max_files_per_commit: 0,
  },
  lease: { id: "L1", holder: "agent1", paths: ["src/auth"], mode: "exclusive", created_ms: 4, expires_ms: 4000, note: "refactor" },
  ids: { A: ID_A, B: ID_B },
};

function json(res: ServerResponse, status: number, body: unknown) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

function error(res: ServerResponse, status: number, code: string, message: string, retryable = false, details?: unknown) {
  json(res, status, { ok: false, error: { code, message, retryable, ...(details !== undefined ? { details } : {}) } });
}

function readBody(req: IncomingMessage): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks)));
    req.on("error", reject);
  });
}

function sseEvent(seq: number, kind: string, data: Record<string, unknown>): string {
  const ev = { seq, time_ms: 1000 + seq, kind, actor: "agent1", data };
  return `id: ${seq}\nevent: ${kind}\ndata: ${JSON.stringify(ev)}\n\n`;
}

export async function startFakeServer(): Promise<FakeServer> {
  const requests: SeenRequest[] = [];
  const state: FakeServer = { baseUrl: "", requests, sseConnections: 0, close: async () => {} };
  const sockets = new Set<import("node:net").Socket>();

  const server: Server = createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", "http://localhost");
    const rawBody = await readBody(req);
    let body: unknown = undefined;
    if (rawBody.length && (req.headers["content-type"] ?? "").includes("json")) {
      try {
        body = JSON.parse(rawBody.toString("utf8"));
      } catch {
        body = rawBody.toString("utf8");
      }
    }
    const seen: SeenRequest = { method: req.method ?? "GET", path: url.pathname, query: url.searchParams, headers: req.headers, body, rawBody };
    requests.push(seen);
    const route = `${seen.method} ${url.pathname}`;

    // Token check: when a token is sent it must be the right one.
    const auth = req.headers.authorization;
    if (auth !== undefined && auth !== "Bearer secret") return error(res, 403, "forbidden", "missing or invalid bearer token");

    switch (true) {
      case route === "GET /api/v1/health":
        return json(res, 200, fixtures.health);
      case route === "GET /api/v1/repos":
        return json(res, 200, fixtures.repos);
      case route === "POST /api/v1/repos": {
        const b = body as { name?: string; default_branch?: string };
        if (b.name === "demo") return error(res, 409, "already_exists", "repository exists");
        return json(res, 201, { name: b.name, default_branch: b.default_branch ?? "main", head: null, created_ms: 9, branches: 0, tags: 0, objects: 0 });
      }
      case route === "GET /api/v1/repos/demo":
        return json(res, 200, fixtures.repos[0]);
      case route === "GET /api/v1/repos/missing":
        return error(res, 404, "not_found", "no such repository: missing");
      case route === "DELETE /api/v1/repos/demo":
        return json(res, 200, { deleted: true });
      case route === "GET /api/v1/repos/demo/refs":
        return json(res, 200, [{ name: "refs/heads/main", id: ID_A }]);
      case route === "POST /api/v1/repos/demo/refs/update": {
        const b = body as { updates: { name: string; expect: unknown; new: string | null }[] };
        const u = b.updates[0];
        if (u && typeof u.expect === "object" && u.expect && "is" in u.expect && (u.expect as { is: string }).is === ID_B) {
          return error(res, 409, "ref_conflict", "ref moved", true, { name: u.name, expected: ID_B, actual: ID_A });
        }
        return json(res, 200, { updated: b.updates.filter((x) => x.new).map((x) => ({ name: x.name, id: x.new })), deleted: b.updates.filter((x) => !x.new).map((x) => x.name) });
      }
      case route === "POST /api/v1/repos/demo/objects/missing":
        return json(res, 200, { missing: (body as { ids: string[] }).ids.filter((id) => id !== ID_A) });
      case route === "POST /api/v1/repos/demo/objects":
        return json(res, 200, { stored: rawBody.length ? 1 : 0, skipped: 0 });
      case route === "POST /api/v1/repos/demo/fetch": {
        const payload = Buffer.from("hello");
        const frame = Buffer.alloc(5 + payload.length);
        frame[0] = 1;
        frame.writeUInt32BE(payload.length, 1);
        payload.copy(frame, 5);
        res.writeHead(200, { "content-type": "application/x-agit-objects" });
        return res.end(frame);
      }
      case route === `GET /api/v1/repos/demo/objects/${ID_A}`:
        res.writeHead(200, { "content-type": "application/octet-stream", "x-agit-kind": "blob" });
        return res.end(Buffer.from("raw"));
      case route === "GET /api/v1/repos/demo/commits":
        return json(res, 200, [{ id: ID_A, tree: ID_B, parents: [], author: { name: "a", email: "a@x" }, time_ms: 1, message: "m" }]);
      case route === "GET /api/v1/repos/demo/events":
        return json(res, 200, [{ seq: 1, time_ms: 1, kind: "commit", data: {} }]);
      case route === "GET /api/v1/repos/demo/events/stream": {
        state.sseConnections += 1;
        const since = url.searchParams.has("since") ? Number(url.searchParams.get("since")) : undefined;
        res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache", connection: "keep-alive" });
        if (state.sseConnections === 1) {
          // First connection: a keepalive comment, three events (one with multi-line data), then close.
          res.write(": keepalive\n\n");
          res.write(sseEvent(1, "commit", { id: ID_A }));
          setTimeout(() => {
            res.write(`id: 2\nevent: lease.claim\ndata: {"seq":2,"time_ms":1002,\ndata: "kind":"lease.claim","data":{"holder":"agent1"}}\n\n`);
            res.write(sseEvent(3, "pull.open", { number: 1 }));
            res.end();
          }, 20);
        } else {
          // Reconnect: resume after `since`.
          const next = (since ?? 0) + 1;
          res.write(`retry: 50\n`);
          res.write(sseEvent(next, "note.create", { id: "n_1" }));
          // keep open until the client goes away
          req.on("close", () => res.end());
        }
        return;
      }
      case route === "GET /api/v1/repos/demo/leases":
        return json(res, 200, [fixtures.lease]);
      case route === "POST /api/v1/repos/demo/leases": {
        const b = body as { holder: string; paths: string[]; mode?: string; ttl_secs?: number; note?: string };
        if (b.paths.includes("src/auth") && b.holder !== "agent1") {
          return error(res, 409, "lease_conflict", "src/auth is held by agent1", false, { path: "src/auth", holder: "agent1", lease_id: "L1" });
        }
        return json(res, 201, { ...fixtures.lease, id: `L${requests.length}`, holder: b.holder, paths: b.paths, mode: b.mode ?? "exclusive", note: b.note ?? null });
      }
      case route === "POST /api/v1/repos/demo/leases/check": {
        const b = body as { holder: string; paths: string[] };
        if (b.paths.includes("src/auth") && b.holder !== "agent1") return error(res, 409, "lease_conflict", "held", false, { path: "src/auth", holder: "agent1", lease_id: "L1" });
        return json(res, 200, { allowed: true, holder: b.holder, paths: b.paths });
      }
      case /^POST \/api\/v1\/repos\/demo\/leases\/[^/]+\/renew$/.test(route):
        return json(res, 200, { ...fixtures.lease, expires_ms: 99_999 });
      case /^DELETE \/api\/v1\/repos\/demo\/leases\/[^/]+$/.test(route): {
        const id = decodeURIComponent(url.pathname.split("/").pop() ?? "");
        if (id === "gone") return error(res, 404, "lease_not_found", "no such lease");
        return json(res, 200, { released: id });
      }
      case route === "GET /api/v1/repos/demo/pulls":
        return json(res, 200, url.searchParams.get("state") === "closed" ? [] : [fixtures.pull]);
      case route === "POST /api/v1/repos/demo/pulls": {
        const b = body as Record<string, unknown>;
        return json(res, 201, { ...fixtures.pull, number: 2, title: b.title, source: b.source, target: b.target ?? "refs/heads/main", intent: b.intent ?? null, task: b.task ?? null, author: { id: req.headers["x-agit-agent"] ?? "anonymous", model: req.headers["x-agit-model"] ?? null } });
      }
      case route === "GET /api/v1/repos/demo/pulls/1":
        return json(res, 200, { pr: fixtures.pull, commits: [], changes: [], mergeable: true, conflicts: [] });
      case route === "GET /api/v1/repos/demo/pulls/404":
        return error(res, 404, "not_found", "no such pull request");
      case route === "POST /api/v1/repos/demo/pulls/1/merge": {
        const b = body as { strategy?: string };
        if (b.strategy === "ff") return error(res, 409, "merge_conflicts", "2 conflicts", false, { count: 2 });
        return json(res, 200, { pr: { ...fixtures.pull, state: "merged", merged_commit: ID_B }, merged_commit: ID_B });
      }
      case route === "POST /api/v1/repos/demo/pulls/1/reviews":
        return json(res, 200, { ...fixtures.pull, reviews: [{ reviewer: req.headers["x-agit-agent"], ...(body as object), time_ms: 5 }] });
      case route === "POST /api/v1/repos/demo/pulls/1/comments":
        return json(res, 201, { id: 7, author: req.headers["x-agit-agent"] ?? "anonymous", path: null, line: null, kind: "note", resolved: false, time_ms: 6, ...(body as object) });
      case route === "POST /api/v1/repos/demo/pulls/1/comments/7/resolve":
        return json(res, 200, { ...fixtures.pull, comments: [{ id: 7, resolved: true }] });
      case route === "POST /api/v1/repos/demo/pulls/1/close":
        return json(res, 200, { ...fixtures.pull, state: "closed" });
      case route === "POST /api/v1/repos/demo/pulls/1/update":
        return json(res, 200, { ...fixtures.pull, ...(body as object) });
      case route === "POST /api/v1/repos/demo/pulls/1/checks":
        return json(res, 200, { ...fixtures.pull, checks: [{ ...(body as object), time_ms: 8 }] });
      case route === "GET /api/v1/repos/demo/knowledge/notes": {
        const { body: _b, ...summary } = fixtures.note;
        return json(res, 200, [{ ...summary, excerpt: fixtures.note.body.slice(0, 200) }]);
      }
      case route === "POST /api/v1/repos/demo/knowledge/notes":
        return json(res, 201, { ...fixtures.note, id: "n_2", ...(body as object), author: req.headers["x-agit-agent"] ?? "anonymous" });
      case route === "GET /api/v1/repos/demo/knowledge/notes/n_1":
        return json(res, 200, fixtures.note);
      case route === "PATCH /api/v1/repos/demo/knowledge/notes/n_1":
        return json(res, 200, { ...fixtures.note, ...(body as object) });
      case route === "DELETE /api/v1/repos/demo/knowledge/notes/n_1":
        return json(res, 200, { deleted: "n_1" });
      case route === "GET /api/v1/repos/demo/knowledge/tags":
        return json(res, 200, [{ tag: "area:auth", count: 1 }, { tag: "kind:gotcha", count: 1 }]);
      case route === "GET /api/v1/repos/demo/knowledge/rules":
        return json(res, 200, [{ pattern: "src/auth/**", tags: ["area:auth"] }]);
      case route === "PUT /api/v1/repos/demo/knowledge/rules":
        return json(res, 200, body);
      case route === "GET /api/v1/repos/demo/policy":
        return json(res, 200, fixtures.policy);
      case route === "PUT /api/v1/repos/demo/policy":
        return json(res, 200, body);
      case route === "GET /api/v1/repos/demo/context":
        if (!url.searchParams.get("path")) return error(res, 400, "invalid", "missing `path`");
        return json(res, 200, { path: url.searchParams.get("path"), notes: [], suggested_tags: ["area:auth"], recent_commits: [], leases: [fixtures.lease], open_pulls: [fixtures.pull] });
      case route === "GET /api/v1/repos/demo/agents":
        return json(res, 200, [{ id: "agent1", model: null, commits: 3, last_seen_ms: 10, open_pulls: 1, leases: 1 }]);
      case route === "GET /api/v1/repos/demo/slow":
        setTimeout(() => json(res, 200, { slow: true }), 500);
        return;
      case route === "GET /api/v1/repos/demo/garbage":
        res.writeHead(502, { "content-type": "text/html" });
        return res.end("<h1>Bad gateway</h1>");
      case route === "GET /api/v1/repos/demo/soft-error":
        // 200 with an error envelope (should still be mapped)
        return json(res, 200, { ok: false, error: { code: "policy_violation", message: "nope", retryable: false } });
      default:
        return error(res, 404, "not_found", `no route ${route}`);
    }
  });

  server.on("connection", (s) => {
    sockets.add(s);
    s.on("close", () => sockets.delete(s));
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
  const { port } = server.address() as AddressInfo;
  state.baseUrl = `http://127.0.0.1:${port}/api/v1`;
  state.close = () =>
    new Promise<void>((resolve) => {
      for (const s of sockets) s.destroy();
      server.close(() => resolve());
    });
  return state;
}
