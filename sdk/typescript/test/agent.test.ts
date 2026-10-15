import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { AgentSession, AgitClient, LeaseConflictError } from "../src/index.js";
import { startFakeServer, type FakeServer } from "./fixtures/server.js";
import { isolateAgitEnv } from "./fixtures/env.js";

isolateAgitEnv();

let server: FakeServer;
let client: AgitClient;

beforeAll(async () => {
  server = await startFakeServer();
  client = new AgitClient({ baseUrl: server.baseUrl, agent: "someone-else" });
});
afterAll(async () => {
  await server.close();
});
beforeEach(() => {
  server.requests.length = 0;
});

const last = () => server.requests[server.requests.length - 1]!;

describe("AgentSession", () => {
  it("acts as its agent id in headers and lease holder", async () => {
    const s = new AgentSession(client, "demo", "agent1", { model: "m1" });
    const lease = await s.claim("src/auth", { ttlSecs: 60, note: "refactor" });
    expect(last().headers["x-agit-agent"]).toBe("agent1");
    expect(last().headers["x-agit-model"]).toBe("m1");
    expect(last().body).toEqual({ holder: "agent1", paths: ["src/auth"], mode: "exclusive", ttl_secs: 60, note: "refactor" });
    expect(s.leases.map((l) => l.id)).toEqual([lease.id]);
    await s.renew(90);
    expect(last().body).toEqual({ ttl_secs: 90 });
    await s.releaseAll();
    expect(last().method).toBe("DELETE");
    expect(last().path).toBe(`/api/v1/repos/demo/leases/${lease.id}`);
    expect(s.leases).toEqual([]);
  });

  it("uses the default ttl and reads context", async () => {
    const s = new AgentSession(client, "demo", "agent1", { defaultTtlSecs: 42 });
    await s.claim(["a", "b"]);
    expect(last().body).toMatchObject({ ttl_secs: 42, paths: ["a", "b"] });
    const ctx = await s.context("src/auth/login.rs");
    expect(last().query.get("path")).toBe("src/auth/login.rs");
    expect(ctx.leases).toHaveLength(1);
  });

  it("withLeases releases in finally even when fn throws", async () => {
    const s = new AgentSession(client, "demo", "agent1");
    await expect(
      s.withLeases("src/x", async (lease) => {
        expect(lease.paths).toEqual(["src/x"]);
        throw new Error("boom");
      }),
    ).rejects.toThrow("boom");
    expect(last().method).toBe("DELETE");
    expect(s.leases).toEqual([]);

    const value = await s.withLeases("src/y", () => 7);
    expect(value).toBe(7);
    const methods = server.requests.slice(-2).map((r) => r.method);
    expect(methods).toEqual(["POST", "DELETE"]);
  });

  it("surfaces lease conflicts from claim and tolerates already-released leases", async () => {
    const s = new AgentSession(client, "demo", "agent2");
    await expect(s.claim("src/auth")).rejects.toBeInstanceOf(LeaseConflictError);
    await expect(s.check("src/auth")).rejects.toBeInstanceOf(LeaseConflictError);
    await expect(s.release("gone")).resolves.toBeUndefined();
  });

  it("opens pull requests and remembers notes", async () => {
    const s = new AgentSession(client, "demo", "agent1");
    const pr = await s.openPullRequest({ title: "T", source: "ws/agent1", target: "main", intent: "why", task: "T-1" });
    expect(last().body).toEqual({ title: "T", source: "refs/heads/ws/agent1", target: "refs/heads/main", intent: "why", task: "T-1" });
    expect(pr.author.id).toBe("agent1");
    const note = await s.remember({ title: "Gotcha", body: "details", tags: ["kind:gotcha"], paths: ["src/auth/**"], commits: ["a".repeat(64)], pulls: [pr.number] });
    expect(last().body).toEqual({ title: "Gotcha", body: "details", tags: ["kind:gotcha"], paths: ["src/auth/**"], commits: ["a".repeat(64)], pulls: [2] });
    expect(note.author).toBe("agent1");
  });

  it("reuses the client when it already acts as the agent", () => {
    const own = new AgitClient({ baseUrl: server.baseUrl, agent: "agent1" });
    const s = new AgentSession(own, "demo", "agent1");
    expect(s.client).toBe(own);
    const other = new AgentSession(own, "demo", "agent9");
    expect(other.client).not.toBe(own);
    expect(other.client.agent).toBe("agent9");
  });
});
