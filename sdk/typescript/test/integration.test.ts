// Runs against a real `agit serve` only when AGIT_SERVER_URL is set, e.g.
//   AGIT_SERVER_URL=http://127.0.0.1:7878/api/v1 pnpm test
// Everything it creates lives in a throwaway repository that is deleted afterwards.

import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { AgentSession, AgitClient, LeaseConflictError, NotFoundError } from "../src/index.js";

const url = process.env.AGIT_SERVER_URL;
const repo = `sdk-ts-it-${Date.now().toString(36)}`;

describe.skipIf(!url)("integration against a live agit server", () => {
  const client = new AgitClient({ baseUrl: url, token: process.env.AGIT_TOKEN, agent: "sdk-ts-tester", model: "test-model" });

  beforeAll(async () => {
    await client.createRepo({ name: repo });
  });
  afterAll(async () => {
    await client.deleteRepo(repo).catch(() => undefined);
  });

  it("health, repo, policy, leases, notes, context, events", async () => {
    const h = await client.health();
    expect(h.status).toBe("ok");

    const detail = await client.getRepo(repo);
    expect(detail.name).toBe(repo);
    expect(detail.default_branch).toBe("main");

    const policy = await client.getPolicy(repo);
    expect(policy.block_on_blockers).toBe(true);
    const saved = await client.setPolicy(repo, { ...policy, require_intent: true });
    expect(saved.require_intent).toBe(true);

    const session = new AgentSession(client, repo, "sdk-ts-tester");
    const lease = await session.claim("src/auth", { ttlSecs: 60, note: "it" });
    expect(lease.holder).toBe("sdk-ts-tester");
    const other = new AgentSession(client, repo, "sdk-ts-other");
    await expect(other.claim("src/auth/login.rs")).rejects.toBeInstanceOf(LeaseConflictError);

    const note = await session.remember({ title: "it note", body: "body", tags: ["area:auth", "kind:test"], paths: ["src/auth/**"] });
    const found = await client.listNotes(repo, { tags: ["area:auth", "kind:test"], path: "src/auth/login.rs" });
    expect(found.map((n) => n.id)).toContain(note.id);
    expect(await client.listNoteTags(repo)).toEqual(expect.arrayContaining([{ tag: "area:auth", count: 1 }]));

    const ctx = await session.context("src/auth/login.rs");
    expect(ctx.leases.map((l) => l.id)).toContain(lease.id);
    expect(ctx.notes.map((n) => n.id)).toContain(note.id);
    expect(ctx.agents_active).toContain("sdk-ts-tester");
    expect(ctx.files).toBeUndefined();
    const pack = await session.contextPack("src/auth/login.rs", 1000);
    expect(pack.files ?? []).toEqual([]); // empty repository: nothing to pack
    expect(pack.notes.map((n) => n.id)).toContain(note.id);

    // knowledge v2: ranked search, drafts, accept, validate, stale
    const hits = await client.listNotes(repo, { q: "note" });
    expect(hits.map((h) => h.id)).toEqual([note.id]);
    expect(hits[0]?.score).toBeGreaterThan(0);
    expect(hits[0]).toMatchObject({ draft: false, stale: false });
    expect(await client.listNotes(repo, { pull: 7 })).toEqual([]);
    const draft = await client.createNote(repo, { title: "draft from sdk", body: "b", draft: true, pulls: [7], paths: ["src/auth/**"] });
    expect(draft.draft).toBe(true);
    expect((await client.listNotes(repo, { path: "src/auth/x.rs" })).map((n) => n.id)).not.toContain(draft.id);
    expect((await client.listNotes(repo, { drafts: true, pull: 7 })).map((n) => n.id)).toEqual([draft.id]);
    const accepted = await session.acceptNote(draft, { title: "accepted from sdk" });
    expect(accepted.draft).toBeUndefined();
    expect(accepted.title).toBe("accepted from sdk");
    expect((await client.listNotes(repo, { pull: 7 })).map((n) => n.id)).toEqual([accepted.id]);
    expect(await session.staleNotes()).toEqual([]); // no commits yet
    await client.deleteNote(repo, accepted.id);

    // signing keys
    const pk = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"; // RFC 8032 test key
    expect(await client.listKeys(repo)).toEqual([]);
    const key = await client.registerKey(repo, { agent: "sdk-ts-tester", public_key: pk });
    expect(key).toMatchObject({ agent: "sdk-ts-tester", public_key: pk, added_by: "sdk-ts-tester" });
    expect((await client.registerKey(repo, { agent: "sdk-ts-tester", public_key: pk })).key_id).toBe(key.key_id);
    await expect(client.registerKey(repo, { agent: "sdk-ts-other", public_key: pk })).rejects.toMatchObject({ code: "already_exists", status: 409 });
    await expect(client.registerKey(repo, { agent: "sdk-ts-tester", public_key: "zz" })).rejects.toMatchObject({ code: "invalid" });
    expect((await client.listKeys(repo)).map((k) => k.key_id)).toEqual([key.key_id]);
    const signedPolicy = await client.setPolicy(repo, { ...saved, require_signed: true });
    expect(signedPolicy.require_signed).toBe(true);
    expect(await client.deleteKey(repo, key.key_id)).toEqual({ deleted: key.key_id });
    await expect(client.deleteKey(repo, key.key_id)).rejects.toBeInstanceOf(NotFoundError);

    const events = await client.listEvents(repo, { since: 0 });
    expect(events.some((e) => e.kind === "lease.claim")).toBe(true);
    for (const kind of ["note.create", "note.accept", "key.add", "key.remove"]) expect(events.some((e) => e.kind === kind)).toBe(true);

    const streamed: string[] = [];
    const ctl = new AbortController();
    const consumer = (async () => {
      for await (const ev of client.streamEvents(repo, { since: 0, signal: ctl.signal })) {
        streamed.push(ev.kind);
        if (ev.kind === "lease.release") break;
      }
    })();
    await session.releaseAll();
    await Promise.race([consumer, new Promise((_, reject) => setTimeout(() => reject(new Error("stream timeout")), 5000))]);
    ctl.abort();
    expect(streamed).toContain("lease.claim");
    expect(streamed).toContain("lease.release");

    await client.deleteNote(repo, note.id);
    await expect(client.getNote(repo, note.id)).rejects.toBeInstanceOf(NotFoundError);
  });
});
