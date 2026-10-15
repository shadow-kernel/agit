import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import {
  AgitClient,
  AgitError,
  LeaseConflictError,
  MergeConflictsError,
  NotFoundError,
  PolicyViolationError,
  RefConflictError,
  decodeObjectStream,
  encodeObjectStream,
  expectAbsent,
  expectAny,
  expectIs,
  isRetryable,
} from "../src/index.js";
import { fixtures, startFakeServer, type FakeServer } from "./fixtures/server.js";
import { isolateAgitEnv } from "./fixtures/env.js";

isolateAgitEnv();

let server: FakeServer;
let client: AgitClient;

beforeAll(async () => {
  server = await startFakeServer();
  client = new AgitClient({ baseUrl: server.baseUrl, token: "secret", agent: "agent1", model: "model-x", timeoutMs: 2000 });
});
afterAll(async () => {
  await server.close();
});
beforeEach(() => {
  server.requests.length = 0;
});

const last = () => server.requests[server.requests.length - 1]!;

describe("headers and URLs", () => {
  it("sends Authorization, X-Agit-Agent and X-Agit-Model on every request", async () => {
    expect(await client.health()).toEqual(fixtures.health);
    const r = last();
    expect(r.path).toBe("/api/v1/health");
    expect(r.headers.authorization).toBe("Bearer secret");
    expect(r.headers["x-agit-agent"]).toBe("agent1");
    expect(r.headers["x-agit-model"]).toBe("model-x");
    expect(r.headers.accept).toBe("application/json");
  });

  it("omits identity headers when not configured and strips trailing slashes from baseUrl", async () => {
    const anon = new AgitClient({ baseUrl: server.baseUrl + "///", fetch: globalThis.fetch });
    await anon.health();
    const r = last();
    expect(r.headers.authorization).toBeUndefined();
    expect(r.headers["x-agit-agent"]).toBeUndefined();
    expect(r.headers["x-agit-model"]).toBeUndefined();
  });

  it("defaults baseUrl to the local server", () => {
    const saved = process.env.AGIT_SERVER_URL;
    delete process.env.AGIT_SERVER_URL;
    try {
      expect(new AgitClient().baseUrl).toBe("http://127.0.0.1:7878/api/v1");
    } finally {
      if (saved !== undefined) process.env.AGIT_SERVER_URL = saved;
    }
  });

  it("reads defaults from the environment", () => {
    const saved = { ...process.env };
    process.env.AGIT_SERVER_URL = "http://example.test/api/v1/";
    process.env.AGIT_TOKEN = "t";
    process.env.AGIT_AGENT = "env-agent";
    process.env.AGIT_MODEL = "env-model";
    try {
      const c = new AgitClient();
      expect(c.baseUrl).toBe("http://example.test/api/v1");
      expect(c.headers()).toEqual({ Authorization: "Bearer t", "X-Agit-Agent": "env-agent", "X-Agit-Model": "env-model" });
    } finally {
      for (const k of ["AGIT_SERVER_URL", "AGIT_TOKEN", "AGIT_AGENT", "AGIT_MODEL"]) {
        if (saved[k] === undefined) delete process.env[k];
        else process.env[k] = saved[k];
      }
    }
  });

  it("encodes repository names and path segments", async () => {
    await expect(client.getRepo("missing")).rejects.toBeInstanceOf(NotFoundError);
    const c2 = new AgitClient({ baseUrl: server.baseUrl, fetch: globalThis.fetch });
    await c2.getNote("demo", "n_1");
    expect(last().path).toBe("/api/v1/repos/demo/knowledge/notes/n_1");
    await c2.getCommit("demo", "feature/x").catch(() => undefined);
    expect(last().path).toBe("/api/v1/repos/demo/commits/feature%2Fx");
  });

  it("sends a JSON content type for bodies and none for bodiless calls", async () => {
    await client.createRepo({ name: "other", default_branch: "trunk" });
    expect(last().headers["content-type"]).toBe("application/json");
    expect(last().body).toEqual({ name: "other", default_branch: "trunk" });
    await client.closePull("demo", 1);
    expect(last().headers["content-type"]).toBeUndefined();
  });
});

describe("query encoding", () => {
  it("joins tags with commas and drops empty values for notes", async () => {
    const notes = await client.listNotes("demo", { tags: ["area:auth", "kind:gotcha"], path: "src/auth/login.rs", q: "", limit: 5 });
    expect(notes[0]?.excerpt).toBe(fixtures.note.body);
    expect((notes[0] as unknown as Record<string, unknown>).body).toBeUndefined();
    const q = last().query;
    expect(q.get("tags")).toBe("area:auth,kind:gotcha");
    expect(q.get("path")).toBe("src/auth/login.rs");
    expect(q.has("q")).toBe(false);
    expect(q.get("limit")).toBe("5");
    expect(q.has("include_superseded")).toBe(false);
  });

  it("passes commit filters and omits undefined", async () => {
    await client.listCommits("demo", { rev: "main", limit: 10, agent: "agent1", first_parent: true });
    const q = last().query;
    expect([...q.keys()].sort()).toEqual(["agent", "first_parent", "limit", "rev"]);
    expect(q.get("first_parent")).toBe("1");
  });

  it("maps pulls state=all to no state param", async () => {
    await client.listPulls("demo", "all");
    expect(last().query.has("state")).toBe(false);
    expect(await client.listPulls("demo", "closed")).toEqual([]);
    expect(last().query.get("state")).toBe("closed");
  });

  it("sends refs prefix, diff from/to, tree path and context path/limit", async () => {
    await client.listRefs("demo", "refs/heads/");
    expect(last().query.get("prefix")).toBe("refs/heads/");
    await client.getDiff("demo", "a", "b").catch(() => undefined);
    expect(last().query.get("from")).toBe("a");
    expect(last().query.get("to")).toBe("b");
    await client.getTree("demo", "main", "src/x").catch(() => undefined);
    expect(last().path).toBe("/api/v1/repos/demo/tree/main");
    expect(last().query.get("path")).toBe("src/x");
    const ctx = await client.getContext("demo", "src/auth/login.rs", 3);
    expect(last().query.get("limit")).toBe("3");
    expect(ctx.suggested_tags).toEqual(["area:auth"]);
    expect(ctx.open_pulls[0]?.number).toBe(1);
  });

  it("lists events with since and limit", async () => {
    await client.listEvents("demo", { since: 7, limit: 100 });
    expect(last().query.get("since")).toBe("7");
    expect(last().query.get("limit")).toBe("100");
  });
});

describe("repos, pulls, knowledge, policy, leases", () => {
  it("lists and reads repos", async () => {
    expect(await client.listRepos()).toEqual(fixtures.repos);
    const repo = await client.getRepo("demo");
    expect(repo.branches).toBe(1);
    expect(await client.deleteRepo("demo")).toEqual({ deleted: true });
    expect(last().method).toBe("DELETE");
  });

  it("creates and reads pull requests", async () => {
    const pr = await client.createPull("demo", { title: "T", source: "refs/heads/ws/agent1", intent: "why", task: "T-1" });
    expect(pr.number).toBe(2);
    expect(pr.author).toEqual({ id: "agent1", model: "model-x" });
    expect(last().body).toEqual({ title: "T", source: "refs/heads/ws/agent1", intent: "why", task: "T-1" });
    const detail = await client.getPull("demo", 1);
    expect(detail.pr.number).toBe(1);
    expect(detail.mergeable).toBe(true);
    await expect(client.getPull("demo", 404)).rejects.toMatchObject({ code: "not_found", status: 404 });
  });

  it("comments, resolves, reviews, checks, updates and merges", async () => {
    const c = await client.commentPull("demo", 1, { path: "src/x.rs", line: 12, kind: "blocker", body: "hm" });
    expect(c.id).toBe(7);
    expect(c.kind).toBe("blocker");
    const afterResolve = await client.resolveComment("demo", 1, 7);
    expect(last().path).toBe("/api/v1/repos/demo/pulls/1/comments/7/resolve");
    expect(afterResolve.comments[0]).toMatchObject({ id: 7, resolved: true });
    const reviewed = await client.reviewPull("demo", 1, { decision: "approve" });
    expect(last().body).toEqual({ decision: "approve", body: "" });
    expect(reviewed.reviews[0]?.decision).toBe("approve");
    const checked = await client.setCheck("demo", 1, { name: "tests", status: "success" });
    expect(checked.checks[0]?.status).toBe("success");
    const updated = await client.updatePull("demo", 1, { title: "New" });
    expect(updated.title).toBe("New");
    const merged = await client.mergePull("demo", 1);
    expect(last().body).toEqual({ strategy: "merge" });
    expect(merged.merged_commit).toBe(fixtures.ids.B);
    expect(merged.pr.state).toBe("merged");
    const err = await client.mergePull("demo", 1, { strategy: "ff" }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(MergeConflictsError);
    expect((err as MergeConflictsError).details).toEqual({ count: 2 });
  });

  it("namespaced accessors call the same endpoints", async () => {
    await client.pulls.get("demo", 1);
    expect(last().path).toBe("/api/v1/repos/demo/pulls/1");
    await client.knowledge.listTags("demo");
    expect(last().path).toBe("/api/v1/repos/demo/knowledge/tags");
    await client.leases.list("demo");
    expect(last().path).toBe("/api/v1/repos/demo/leases");
  });

  it("knowledge CRUD, tags and rules", async () => {
    const n = await client.createNote("demo", { title: "x", body: "y", tags: ["area:auth"], paths: ["src/**"] });
    expect(n.id).toBe("n_2");
    expect(n.author).toBe("agent1");
    const patched = await client.updateNote("demo", "n_1", { superseded_by: null, confidence: 0.5 });
    expect(last().method).toBe("PATCH");
    expect(last().body).toEqual({ superseded_by: null, confidence: 0.5 });
    expect(patched.confidence).toBe(0.5);
    expect(await client.deleteNote("demo", "n_1")).toEqual({ deleted: "n_1" });
    expect(await client.listNoteTags("demo")).toEqual([
      { tag: "area:auth", count: 1 },
      { tag: "kind:gotcha", count: 1 },
    ]);
    expect(await client.getTagRules("demo")).toEqual([{ pattern: "src/auth/**", tags: ["area:auth"] }]);
    const rules = [{ pattern: "web/**", tags: ["area:web"] }];
    expect(await client.setTagRules("demo", rules)).toEqual(rules);
    expect(last().method).toBe("PUT");
  });

  it("policy GET and PUT round-trip", async () => {
    expect(await client.getPolicy("demo")).toEqual(fixtures.policy);
    const next = { ...fixtures.policy, require_approvals: 2 };
    expect(await client.setPolicy("demo", next)).toEqual(next);
    expect(last().method).toBe("PUT");
    expect(last().body).toEqual(next);
  });

  it("leases: claim, renew, check, release and 409 envelope", async () => {
    const l = await client.claimLease("demo", { holder: "agent1", paths: ["src/auth"], ttl_secs: 60, note: "n" });
    expect(l.holder).toBe("agent1");
    expect(last().body).toEqual({ holder: "agent1", paths: ["src/auth"], ttl_secs: 60, note: "n" });
    const renewed = await client.renewLease("demo", l.id, 120);
    expect(last().body).toEqual({ ttl_secs: 120 });
    expect(renewed.expires_ms).toBe(99_999);
    expect(await client.checkLease("demo", "agent1", ["src/auth"])).toMatchObject({ allowed: true });
    expect(await client.releaseLease("demo", l.id)).toEqual({ released: l.id });

    const err = await client.claimLease("demo", { holder: "agent2", paths: ["src/auth"] }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LeaseConflictError);
    const le = err as LeaseConflictError;
    expect(le.code).toBe("lease_conflict");
    expect(le.status).toBe(409);
    expect(le.retryable).toBe(false);
    expect(le.details).toEqual({ path: "src/auth", holder: "agent1", lease_id: "L1" });
    expect(le.message).toBe("src/auth is held by agent1");
    await expect(client.checkLease("demo", "agent2", ["src/auth"])).rejects.toBeInstanceOf(LeaseConflictError);
  });

  it("agents and refs", async () => {
    expect((await client.listAgents("demo"))[0]?.id).toBe("agent1");
    expect(await client.listRefs("demo")).toEqual([{ name: "refs/heads/main", id: fixtures.ids.A }]);
  });
});

describe("refs compare-and-swap", () => {
  it("builds expect specs and maps ref_conflict", async () => {
    expect(expectIs("x")).toEqual({ is: "x" });
    expect(expectAbsent()).toEqual({ absent: true });
    expect(expectAny()).toEqual({ any: true });
    const r = await client.updateRefs("demo", [{ name: "refs/heads/main", expect: expectIs(fixtures.ids.A), new: fixtures.ids.B }, { name: "refs/heads/old", expect: expectAny(), new: null }], "reason");
    expect(last().body).toEqual({ updates: [{ name: "refs/heads/main", expect: { is: fixtures.ids.A }, new: fixtures.ids.B }, { name: "refs/heads/old", expect: { any: true }, new: null }], message: "reason" });
    expect(r.updated).toEqual([{ name: "refs/heads/main", id: fixtures.ids.B }]);
    expect(r.deleted).toEqual(["refs/heads/old"]);

    const err = await client.updateRefs("demo", [{ name: "refs/heads/main", expect: expectIs(fixtures.ids.B), new: fixtures.ids.A }]).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(RefConflictError);
    expect(isRetryable(err)).toBe(true);
    expect((err as RefConflictError).details).toEqual({ name: "refs/heads/main", expected: fixtures.ids.B, actual: fixtures.ids.A });
  });
});

describe("objects", () => {
  it("uploads with the object stream content type and fetches a decodable stream", async () => {
    const stream = encodeObjectStream([{ kind: "blob", payload: new TextEncoder().encode("hi") }]);
    expect(await client.uploadObjects("demo", stream)).toEqual({ stored: 1, skipped: 0 });
    expect(last().headers["content-type"]).toBe("application/x-agit-objects");
    expect(new Uint8Array(last().rawBody)).toEqual(stream);

    const fetched = await client.fetchObjects("demo", [fixtures.ids.A], [fixtures.ids.B]);
    expect(last().body).toEqual({ wants: [fixtures.ids.A], haves: [fixtures.ids.B] });
    const frames = decodeObjectStream(fetched);
    expect(frames).toHaveLength(1);
    expect(frames[0]?.kind).toBe("blob");
    expect(new TextDecoder().decode(frames[0]?.payload)).toBe("hello");

    expect(await client.missingObjects("demo", [fixtures.ids.A, fixtures.ids.B])).toEqual([fixtures.ids.B]);
    const raw = await client.getObject("demo", fixtures.ids.A);
    expect(raw.kind).toBe("blob");
    expect(new TextDecoder().decode(raw.payload)).toBe("raw");
  });
});

describe("error mapping", () => {
  it("maps codes to subclasses", async () => {
    await expect(client.getRepo("missing")).rejects.toBeInstanceOf(NotFoundError);
    const dup = await client.createRepo({ name: "demo" }).catch((e: unknown) => e);
    expect(dup).toBeInstanceOf(AgitError);
    expect(dup).not.toBeInstanceOf(NotFoundError);
    expect((dup as AgitError).code).toBe("already_exists");
    expect((dup as AgitError).status).toBe(409);
  });

  it("maps a 403 from a bad token", async () => {
    const bad = new AgitClient({ baseUrl: server.baseUrl, token: "wrong" });
    const err = await bad.health().catch((e: unknown) => e);
    expect(err).toMatchObject({ code: "forbidden", status: 403, retryable: false });
  });

  it("handles non-JSON error bodies and 200 responses carrying an error envelope", async () => {
    const err = await client.request("GET", "/repos/demo/garbage").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AgitError);
    expect((err as AgitError).code).toBe("http_502");
    expect((err as AgitError).retryable).toBe(true);
    const soft = await client.request("GET", "/repos/demo/soft-error").catch((e: unknown) => e);
    expect(soft).toBeInstanceOf(PolicyViolationError);
  });

  it("reports network failures and timeouts as retryable transport errors", async () => {
    const dead = new AgitClient({ baseUrl: "http://127.0.0.1:1/api/v1", timeoutMs: 2000 });
    const err = await dead.health().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AgitError);
    expect((err as AgitError).code).toBe("network");
    expect((err as AgitError).isTransport).toBe(true);
    expect(isRetryable(err)).toBe(true);

    const slow = await client.request("GET", "/repos/demo/slow", { timeoutMs: 50 }).catch((e: unknown) => e);
    expect((slow as AgitError).code).toBe("timeout");
    expect(isRetryable(slow)).toBe(true);
  });

  it("propagates caller aborts untouched", async () => {
    const ctl = new AbortController();
    const p = client.request("GET", "/repos/demo/slow", { signal: ctl.signal });
    ctl.abort();
    const err = await p.catch((e: unknown) => e);
    expect(err).toBeInstanceOf(Error);
    expect((err as Error).name).toBe("AbortError");
    expect(isRetryable(err)).toBe(false);
  });
});

describe("streamEvents", () => {
  it("parses SSE frames, resumes from the last seq after the server closes, and stops on break", async () => {
    const seen: number[] = [];
    const kinds: string[] = [];
    for await (const ev of client.streamEvents("demo", { since: 0, retryMs: 10 })) {
      seen.push(ev.seq);
      kinds.push(ev.kind);
      if (seen.length === 4) break;
    }
    expect(seen).toEqual([1, 2, 3, 4]);
    expect(kinds).toEqual(["commit", "lease.claim", "pull.open", "note.create"]);
    const streams = server.requests.filter((r) => r.path.endsWith("/events/stream"));
    expect(streams).toHaveLength(2);
    expect(streams[0]?.query.get("since")).toBe("0");
    expect(streams[1]?.query.get("since")).toBe("3");
    expect(streams[0]?.headers.accept).toBe("text/event-stream");
    expect(streams[0]?.headers.authorization).toBe("Bearer secret");
  });

  it("ends when the signal is aborted and throws on non-retryable server errors", async () => {
    const ctl = new AbortController();
    const it = client.streamEvents("demo", { signal: ctl.signal, retryMs: 10 });
    const first = await it.next();
    expect(first.done).toBe(false);
    ctl.abort();
    const rest = await it.next();
    expect(rest.done).toBe(true);

    const gen = client.streamEvents("missing", { retryMs: 10 });
    await expect(gen.next()).rejects.toBeInstanceOf(NotFoundError);
  });
});
