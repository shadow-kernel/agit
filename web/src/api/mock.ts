// In-memory mock of the agit server API (docs/API.md) with realistic fixtures.
// Enabled with VITE_AGIT_MOCK=1 or automatically when GET /health fails.

import type { Api, StreamStatus } from "./client";
import { ApiError, notifyAuthDenied } from "./client";
import { loadSettings } from "./settings";
import type * as T from "./types";
import { DEFAULT_POLICY as T_DEFAULT_POLICY } from "./types";
import { computeBlame } from "../lib/blame";

// ---- helpers -----------------------------------------------------------------

function hashId(seed: string): string {
  // Deterministic 64-hex id (not BLAKE3, but shaped like one).
  let h1 = 0x811c9dc5, h2 = 0x01000193, h3 = 0xdeadbeef, h4 = 0x9e3779b9;
  for (let i = 0; i < seed.length; i++) {
    const c = seed.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 0x01000193) >>> 0;
    h2 = Math.imul(h2 + c, 0x85ebca6b) >>> 0;
    h3 = (Math.imul(h3 ^ (c << 5), 0xc2b2ae35) ^ (h3 >>> 13)) >>> 0;
    h4 = Math.imul(h4 ^ c ^ (h4 >>> 7), 0x27d4eb2f) >>> 0;
  }
  let out = "";
  let s = [h1, h2, h3, h4];
  while (out.length < 64) {
    s = s.map((x, i) => (Math.imul(x ^ (s[(i + 1) % 4] >>> 3), 0x9e3779b1) + i) >>> 0);
    out += s.map((x) => x.toString(16).padStart(8, "0")).join("");
  }
  return out.slice(0, 64);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const latency = () => sleep(80 + Math.random() * 220);
const NOW = Date.now();
const HOUR = 3_600_000;
const DAY = 24 * HOUR;

function notFound(what: string): never {
  throw new ApiError(404, "not_found", `${what} not found`, false);
}

// LCS line diff -> unified hunks with 3 lines of context.
function unifiedDiff(path: string, oldText: string | null, newText: string | null): string {
  const a = oldText === null ? [] : oldText.replace(/\n$/, "").split("\n");
  const b = newText === null ? [] : newText.replace(/\n$/, "").split("\n");
  const n = a.length, m = b.length;
  const dp: Uint16Array[] = [];
  for (let i = 0; i <= n; i++) dp.push(new Uint16Array(m + 1));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  type Op = { t: " " | "-" | "+"; s: string; ai: number; bi: number };
  const ops: Op[] = [];
  let i = 0, j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) ops.push({ t: " ", s: a[i], ai: i++, bi: j++ });
    else if (j < m && (i >= n || dp[i][j + 1] >= dp[i + 1][j])) ops.push({ t: "+", s: b[j], ai: i, bi: j++ });
    else ops.push({ t: "-", s: a[i], ai: i++, bi: j });
  }
  const header = `diff --git a/${path} b/${path}\n--- ${oldText === null ? "/dev/null" : "a/" + path}\n+++ ${newText === null ? "/dev/null" : "b/" + path}\n`;
  const C = 3;
  const changed = ops.map((o) => o.t !== " ");
  const include = new Array(ops.length).fill(false);
  for (let k = 0; k < ops.length; k++) if (changed[k]) for (let d = -C; d <= C; d++) if (ops[k + d]) include[k + d] = true;
  let out = header;
  let k = 0;
  while (k < ops.length) {
    if (!include[k]) { k++; continue; }
    let e = k;
    while (e < ops.length && include[e]) e++;
    const slice = ops.slice(k, e);
    const oldStart = slice[0].ai + 1, newStart = slice[0].bi + 1;
    const oldLen = slice.filter((o) => o.t !== "+").length, newLen = slice.filter((o) => o.t !== "-").length;
    out += `@@ -${oldLen === 0 ? oldStart - 1 : oldStart},${oldLen} +${newLen === 0 ? newStart - 1 : newStart},${newLen} @@\n`;
    for (const o of slice) out += `${o.t}${o.s}\n`;
    k = e;
  }
  return out;
}

function globToRegex(glob: string): RegExp {
  let re = "";
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === "*") {
      if (glob[i + 1] === "*") { re += ".*"; i++; if (glob[i + 1] === "/") i++; } else re += "[^/]*";
    } else if (c === "?") re += "[^/]";
    else re += c.replace(/[.+^${}()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${re}$`);
}
function patternCovers(pattern: string, path: string): boolean {
  if (/[*?[]/.test(pattern)) return globToRegex(pattern).test(path);
  return path === pattern || path.startsWith(pattern.replace(/\/$/, "") + "/");
}

/** Line-set Jaccard similarity in percent (what `agit_core::diff::detect_renames` uses for text files). */
function similarity(a: string, b: string): number {
  if (a === b) return 100;
  const sa = new Set(a.split("\n")), sb = new Set(b.split("\n"));
  let inter = 0;
  for (const l of sa) if (sb.has(l)) inter++;
  const union = sa.size + sb.size - inter;
  return union === 0 ? 100 : Math.round((inter / union) * 100);
}

/** Pair deletions with additions that look like the same file moved (>= 50% similar), like the CLI does. */
function detectRenames(changes: T.Change[], textOf: (path: string, side: "old" | "new") => string | null): T.Change[] {
  const deleted = changes.filter((c) => c.kind === "deleted");
  const added = changes.filter((c) => c.kind === "added");
  if (!deleted.length || !added.length) return changes;
  const rest = changes.filter((c) => c.kind !== "deleted" && c.kind !== "added");
  const usedAdd = new Set<string>();
  const out: T.Change[] = [...rest];
  for (const d of deleted) {
    let best: { c: T.Change; sim: number } | null = null;
    for (const a of added) {
      if (usedAdd.has(a.path)) continue;
      const sim = similarity(textOf(d.path, "old") ?? "", textOf(a.path, "new") ?? "");
      if (sim >= 50 && (!best || sim > best.sim)) best = { c: a, sim };
    }
    if (best) {
      usedAdd.add(best.c.path);
      out.push({ path: best.c.path, kind: "renamed", old: d.old, new: best.c.new, from: d.path, similarity: best.sim });
    } else out.push(d);
  }
  for (const a of added) if (!usedAdd.has(a.path)) out.push(a);
  return out.sort((a, b) => a.path.localeCompare(b.path));
}

function patchFor(changes: T.Change[], textOf: (path: string, side: "old" | "new") => string | null): string {
  let patch = "";
  for (const c of changes) {
    if (c.kind === "renamed" && c.from) {
      const o = textOf(c.from, "old"), n = textOf(c.path, "new");
      if (o === n) continue; // pure rename: no hunks
      patch += unifiedDiff(c.path, o, n).replace(`diff --git a/${c.path} b/${c.path}\n--- a/${c.path}`, `diff --git a/${c.from} b/${c.path}\n--- a/${c.from}`);
    } else patch += unifiedDiff(c.path, c.kind === "added" ? null : textOf(c.path, "old"), c.kind === "deleted" ? null : textOf(c.path, "new"));
  }
  return patch;
}

// ---- file contents -----------------------------------------------------------

const F = {
  readme1: `# acme/shop

Reference storefront service used to dogfood **agit**. Written in TypeScript,
developed almost entirely by agents coordinating through leases and pull requests.

## Layout

| path | what |
| --- | --- |
| \`src/auth\` | login, sessions, token rotation |
| \`src/billing\` | invoices and tax computation |
| \`src/api\` | HTTP server and routes |
| \`tests\` | unit tests (vitest) |

## Running

\`\`\`sh
pnpm install
pnpm dev
\`\`\`

See \`docs/ARCHITECTURE.md\` for the data flow and the knowledge base for gotchas
(\`agit know query --path src/auth\`).
`,
  readme0: `# acme/shop

Reference storefront service used to dogfood agit.
`,
  pkg: `{
  "name": "@acme/shop",
  "version": "0.4.2",
  "private": true,
  "scripts": {
    "dev": "tsx watch src/index.ts",
    "test": "vitest run",
    "build": "tsc -p ."
  },
  "dependencies": {
    "fastify": "^5.3.0",
    "zod": "^3.24.0"
  }
}
`,
  index: `import { createServer } from "./api/server";

const port = Number(process.env.PORT ?? 3000);

createServer()
  .listen({ port })
  .then(() => console.log(\`shop listening on :\${port}\`))
  .catch((err) => {
    console.error(err);
    process.exit(1);
  });
`,
  login0: `import { Session, SessionStore } from "./session";

export interface LoginRequest {
  email: string;
  password: string;
}

export async function login(store: SessionStore, req: LoginRequest): Promise<Session> {
  const user = await store.findUser(req.email);
  if (!user) throw new Error("invalid credentials");
  if (!(await store.verifyPassword(user, req.password))) {
    throw new Error("invalid credentials");
  }
  return store.createSession(user.id);
}
`,
  login1: `import { Session, SessionStore } from "./session";
import { AuthError } from "./errors";

export interface LoginRequest {
  email: string;
  password: string;
}

const MAX_ATTEMPTS = 5;

export async function login(store: SessionStore, req: LoginRequest): Promise<Session> {
  const user = await store.findUser(req.email.trim().toLowerCase());
  if (!user) throw new AuthError("invalid_credentials");
  if (user.failedAttempts >= MAX_ATTEMPTS) {
    throw new AuthError("locked", { retryAfterSecs: 900 });
  }
  if (!(await store.verifyPassword(user, req.password))) {
    await store.recordFailure(user.id);
    throw new AuthError("invalid_credentials");
  }
  await store.resetFailures(user.id);
  return store.createSession(user.id);
}
`,
  login2: `import { Session, SessionStore } from "./session";
import { AuthError } from "./errors";
import { audit } from "../audit";

export interface LoginRequest {
  email: string;
  password: string;
}

const MAX_ATTEMPTS = 5;
const LOCK_SECS = 900;

export async function login(store: SessionStore, req: LoginRequest): Promise<Session> {
  const email = req.email.trim().toLowerCase();
  const user = await store.findUser(email);
  if (!user) throw new AuthError("invalid_credentials");
  if (user.failedAttempts >= MAX_ATTEMPTS) {
    audit("login.locked", { userId: user.id });
    throw new AuthError("locked", { retryAfterSecs: LOCK_SECS });
  }
  if (!(await store.verifyPassword(user, req.password))) {
    await store.recordFailure(user.id);
    throw new AuthError("invalid_credentials");
  }
  await store.resetFailures(user.id);
  const session = await store.createSession(user.id);
  audit("login.ok", { userId: user.id, sessionId: session.id });
  return session;
}
`,
  session0: `export interface Session {
  id: string;
  userId: string;
  createdAt: number;
  expiresAt: number;
}

export interface User {
  id: string;
  email: string;
  passwordHash: string;
  failedAttempts: number;
}

export interface SessionStore {
  findUser(email: string): Promise<User | null>;
  verifyPassword(user: User, password: string): Promise<boolean>;
  createSession(userId: string): Promise<Session>;
  recordFailure(userId: string): Promise<void>;
  resetFailures(userId: string): Promise<void>;
}

export const SESSION_TTL_MS = 24 * 60 * 60 * 1000;
`,
  session1: `export interface Session {
  id: string;
  userId: string;
  createdAt: number;
  expiresAt: number;
}

export interface User {
  id: string;
  email: string;
  passwordHash: string;
  failedAttempts: number;
}

export interface SessionStore {
  findUser(email: string): Promise<User | null>;
  verifyPassword(user: User, password: string): Promise<boolean>;
  createSession(userId: string): Promise<Session>;
  recordFailure(userId: string): Promise<void>;
  resetFailures(userId: string): Promise<void>;
  /** Invalidate every session of a user. Called on password change. */
  rotate(userId: string): Promise<void>;
}

export const SESSION_TTL_MS = 24 * 60 * 60 * 1000;

export function isExpired(s: Session, now = Date.now()): boolean {
  return s.expiresAt <= now;
}
`,
  errors: `export type AuthCode = "invalid_credentials" | "locked" | "expired";

export class AuthError extends Error {
  constructor(public code: AuthCode, public details: Record<string, unknown> = {}) {
    super(code);
    this.name = "AuthError";
  }
}
`,
  audit: `export type AuditEvent = "login.ok" | "login.locked" | "invoice.create" | "invoice.void";

const sink: { event: AuditEvent; data: unknown; at: number }[] = [];

export function audit(event: AuditEvent, data: Record<string, unknown>) {
  sink.push({ event, data, at: Date.now() });
  if (sink.length > 10_000) sink.shift();
}

export function drain() {
  return sink.splice(0);
}
`,
  invoice0: `export interface LineItem {
  sku: string;
  qty: number;
  unitCents: number;
}

export interface Invoice {
  id: string;
  items: LineItem[];
  totalCents: number;
}

export function total(items: LineItem[]): number {
  return items.reduce((sum, i) => sum + i.qty * i.unitCents, 0);
}
`,
  invoice1: `import { taxFor } from "./tax";

export interface LineItem {
  sku: string;
  qty: number;
  unitCents: number;
}

export interface Invoice {
  id: string;
  country: string;
  items: LineItem[];
  subtotalCents: number;
  taxCents: number;
  totalCents: number;
}

export function subtotal(items: LineItem[]): number {
  return items.reduce((sum, i) => sum + i.qty * i.unitCents, 0);
}

export function buildInvoice(id: string, country: string, items: LineItem[]): Invoice {
  const subtotalCents = subtotal(items);
  const taxCents = Math.round(subtotalCents * taxFor(country));
  return { id, country, items, subtotalCents, taxCents, totalCents: subtotalCents + taxCents };
}
`,
  invoice2: `import { taxFor } from "./tax";
import { audit } from "../audit";

export interface LineItem {
  sku: string;
  qty: number;
  unitCents: number;
}

export interface Invoice {
  id: string;
  country: string;
  items: LineItem[];
  subtotalCents: number;
  taxCents: number;
  totalCents: number;
  voided?: boolean;
}

export function subtotal(items: LineItem[]): number {
  return items.reduce((sum, i) => sum + i.qty * i.unitCents, 0);
}

export function buildInvoice(id: string, country: string, items: LineItem[]): Invoice {
  if (items.length === 0) throw new Error("invoice must have at least one line item");
  const subtotalCents = subtotal(items);
  const taxCents = Math.round(subtotalCents * taxFor(country));
  const inv = { id, country, items, subtotalCents, taxCents, totalCents: subtotalCents + taxCents };
  audit("invoice.create", { id, totalCents: inv.totalCents });
  return inv;
}

export function voidInvoice(inv: Invoice): Invoice {
  audit("invoice.void", { id: inv.id });
  return { ...inv, voided: true };
}
`,
  tax0: `const RATES: Record<string, number> = {
  DE: 0.19,
  AT: 0.2,
  FR: 0.2,
  US: 0,
};

export function taxFor(country: string): number {
  return RATES[country] ?? 0;
}
`,
  tax1: `const RATES: Record<string, number> = {
  DE: 0.19,
  AT: 0.2,
  FR: 0.2,
  IT: 0.22,
  ES: 0.21,
  NL: 0.21,
  US: 0,
};

export function taxFor(country: string): number {
  const rate = RATES[country.toUpperCase()];
  if (rate === undefined) throw new Error(\`unknown tax jurisdiction: \${country}\`);
  return rate;
}
`,
  taxConflict: `const RATES: Record<string, number> = {
  DE: 0.19,
  AT: 0.2,
  FR: 0.2,
  CH: 0.081,
  US: 0,
};

export function taxFor(country: string): number {
  return RATES[country] ?? 0;
}

export function knownJurisdictions(): string[] {
  return Object.keys(RATES);
}
`,
  server0: `import Fastify from "fastify";

export function createServer() {
  const app = Fastify({ logger: true });
  app.get("/health", async () => ({ status: "ok" }));
  return app;
}
`,
  server1: `import Fastify from "fastify";
import { z } from "zod";
import { login } from "../auth/login";
import { memoryStore } from "../auth/store";

const LoginBody = z.object({ email: z.string().email(), password: z.string().min(8) });

export function createServer() {
  const app = Fastify({ logger: true });
  const store = memoryStore();

  app.get("/health", async () => ({ status: "ok" }));

  app.post("/login", async (req, reply) => {
    const body = LoginBody.parse(req.body);
    const session = await login(store, body);
    reply.code(201);
    return { session };
  });

  return app;
}
`,
  server2: `import Fastify from "fastify";
import { z } from "zod";
import { login } from "../auth/login";
import { memoryStore } from "../auth/store";
import { AuthError } from "../auth/errors";
import { buildInvoice } from "../billing/invoice";

const LoginBody = z.object({ email: z.string().email(), password: z.string().min(8) });
const InvoiceBody = z.object({
  country: z.string().length(2),
  items: z.array(z.object({ sku: z.string(), qty: z.number().int().positive(), unitCents: z.number().int() })),
});

export function createServer() {
  const app = Fastify({ logger: true });
  const store = memoryStore();

  app.setErrorHandler((err, _req, reply) => {
    if (err instanceof AuthError) {
      reply.code(err.code === "locked" ? 423 : 401).send({ error: err.code, ...err.details });
      return;
    }
    reply.send(err);
  });

  app.get("/health", async () => ({ status: "ok" }));

  app.post("/login", async (req, reply) => {
    const body = LoginBody.parse(req.body);
    const session = await login(store, body);
    reply.code(201);
    return { session };
  });

  app.post("/invoices", async (req, reply) => {
    const body = InvoiceBody.parse(req.body);
    const invoice = buildInvoice(crypto.randomUUID(), body.country, body.items);
    reply.code(201);
    return { invoice };
  });

  return app;
}
`,
  store: `import { Session, SessionStore, SESSION_TTL_MS, User } from "./session";

export function memoryStore(): SessionStore {
  const users = new Map<string, User>();
  const sessions = new Map<string, Session>();
  return {
    async findUser(email) {
      return users.get(email) ?? null;
    },
    async verifyPassword(user, password) {
      return user.passwordHash === \`plain:\${password}\`;
    },
    async createSession(userId) {
      const now = Date.now();
      const s: Session = { id: crypto.randomUUID(), userId, createdAt: now, expiresAt: now + SESSION_TTL_MS };
      sessions.set(s.id, s);
      return s;
    },
    async recordFailure(userId) {
      for (const u of users.values()) if (u.id === userId) u.failedAttempts++;
    },
    async resetFailures(userId) {
      for (const u of users.values()) if (u.id === userId) u.failedAttempts = 0;
    },
    async rotate(userId) {
      for (const [id, s] of sessions) if (s.userId === userId) sessions.delete(id);
    },
  };
}
`,
  authTest0: `import { describe, expect, it } from "vitest";
import { login } from "../src/auth/login";
import { memoryStore } from "../src/auth/store";

describe("login", () => {
  it("rejects unknown users", async () => {
    await expect(login(memoryStore(), { email: "x@y.z", password: "secret123" })).rejects.toThrow();
  });
});
`,
  authTest1: `import { describe, expect, it } from "vitest";
import { login } from "../src/auth/login";
import { memoryStore } from "../src/auth/store";
import { AuthError } from "../src/auth/errors";

describe("login", () => {
  it("rejects unknown users", async () => {
    await expect(login(memoryStore(), { email: "x@y.z", password: "secret123" })).rejects.toThrow(AuthError);
  });

  it("normalizes email case", async () => {
    const store = memoryStore();
    await expect(login(store, { email: "  X@Y.Z ", password: "secret123" })).rejects.toMatchObject({
      code: "invalid_credentials",
    });
  });
});
`,
  billingTest: `import { describe, expect, it } from "vitest";
import { buildInvoice } from "../src/billing/invoice";

describe("buildInvoice", () => {
  it("adds German VAT", () => {
    const inv = buildInvoice("i1", "DE", [{ sku: "A", qty: 2, unitCents: 1000 }]);
    expect(inv.taxCents).toBe(380);
    expect(inv.totalCents).toBe(2380);
  });

  it("refuses empty invoices", () => {
    expect(() => buildInvoice("i2", "DE", [])).toThrow();
  });
});
`,
  arch: `# Architecture

\`\`\`
request -> fastify -> zod schema -> domain fn (auth | billing) -> store
\`\`\`

- Domain functions are pure where possible and take their store as a parameter.
- Errors are typed (\`AuthError\`), never strings, so the HTTP layer maps codes to status.
- Audit events go to an in-memory sink in dev; production wires \`drain()\` to the log shipper.
`,
  arch2: `# ADR 0001: Architecture

Status: accepted

\`\`\`
request -> fastify -> zod schema -> domain fn (auth | billing) -> store
\`\`\`

- Domain functions are pure where possible and take their store as a parameter.
- Errors are typed (\`AuthError\`), never strings, so the HTTP layer maps codes to status.
- Audit events go to an in-memory sink in dev; production wires \`drain()\` to the log shipper.
`,
  gitignore: `node_modules
dist
.env
`,
  ci: `name: ci
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: corepack enable && pnpm install --frozen-lockfile
      - run: pnpm test
`,
  // docs-site repo
  docsReadme: `# agit docs site

Static site for the agit documentation. Markdown in \`content/\`, built with a
tiny script into \`dist/\`.
`,
  docsIndex: `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>agit</title>
    <link rel="stylesheet" href="styles.css" />
  </head>
  <body>
    <main id="content"></main>
    <script type="module" src="build.js"></script>
  </body>
</html>
`,
  docsCss: `:root {
  --fg: #1f2328;
  --bg: #ffffff;
  --accent: #0969da;
}
body {
  margin: 0 auto;
  max-width: 72ch;
  padding: 2rem 1rem;
  font: 16px/1.6 system-ui, sans-serif;
  color: var(--fg);
  background: var(--bg);
}
a { color: var(--accent); }
`,
  docsIntro: `# Introduction

agit keeps the git workflow and rebuilds the internals for agents: structured
output, compare-and-swap refs, leases, workspaces, checkpoints and an event log.
`,
  docsBuild: `import { readFile, writeFile, readdir } from "node:fs/promises";
import { marked } from "marked";

const files = await readdir("content");
let html = "";
for (const f of files.sort()) html += marked.parse(await readFile(\`content/\${f}\`, "utf8"));
await writeFile("dist/index.html", html);
`,
};

// ---- history construction -----------------------------------------------------

interface Step {
  msg: string;
  intent?: string;
  task?: string;
  agent?: T.AgentInfo;
  author?: T.Signature;
  at: number; // ms
  changes: Record<string, string | null>;
  checkpoint?: boolean;
  meta?: Record<string, unknown>;
  /** Fixture for `CommitDetail.signature`. */
  sig?: "trusted" | "untrusted" | "invalid";
}

interface BuiltCommit {
  id: string;
  commit: T.Commit;
  changes: T.Change[];
  patch: string;
  snapshot: Map<string, string>;
  signature: T.SignatureInfo;
}

/** Registered signing keys (per repository) and the key ids the fixtures sign with. */
const KEYS: Record<string, { key_id: string; public_key: string }> = {
  "planner-1": { key_id: "k_" + hashId("key:planner-1").slice(0, 16), public_key: hashId("pub:planner-1") },
  "reviewer-1": { key_id: "k_" + hashId("key:reviewer-1").slice(0, 16), public_key: hashId("pub:reviewer-1") },
  "coder-1": { key_id: "k_" + hashId("key:coder-1").slice(0, 16), public_key: hashId("pub:coder-1") },
  "coder-2": { key_id: "k_" + hashId("key:coder-2").slice(0, 16), public_key: hashId("pub:coder-2") },
};

function signatureFor(st: Step, signer: string): T.SignatureInfo {
  const k = KEYS[signer] ?? { key_id: "k_" + hashId(`key:${signer}`).slice(0, 16), public_key: hashId(`pub:${signer}`) };
  switch (st.sig) {
    case "trusted":
      return { status: "valid", key_id: k.key_id, signer, public_key: k.public_key, trusted: true };
    case "untrusted":
      return { status: "valid", key_id: k.key_id, signer, public_key: k.public_key, trusted: false };
    case "invalid":
      return { status: "invalid", reason: "signature does not verify against the embedded public key (commit payload changed after signing)", trusted: false };
    default:
      return { status: "unsigned", trusted: false };
  }
}

const agents = {
  planner: { id: "planner-1", model: "claude-sonnet-4-5", run_id: "run-1001" },
  coder1: { id: "coder-1", model: "claude-sonnet-4-5", run_id: "run-1042" },
  coder2: { id: "coder-2", model: "gpt-5-codex", run_id: "run-1043", on_behalf_of: "daniel" },
  reviewer: { id: "reviewer-1", model: "claude-opus-4-1", run_id: "run-1050" },
  tester: { id: "tester-3", model: "qwen3-coder-480b", run_id: "run-1077" },
} satisfies Record<string, T.AgentInfo>;

const daniel: T.Signature = { name: "Daniel", email: "daniel@example.com" };
const bot = (a: T.AgentInfo): T.Signature => ({ name: a.id, email: `${a.id}@agents.local` });

function buildHistory(repo: string, steps: Step[], start: Map<string, string> = new Map(), parentId: string | null = null): BuiltCommit[] {
  const out: BuiltCommit[] = [];
  let snap = new Map(start);
  let parent = parentId;
  steps.forEach((st, i) => {
    const next = new Map(snap);
    let changes: T.Change[] = [];
    for (const [path, content] of Object.entries(st.changes)) {
      const before = snap.get(path) ?? null;
      if (content === null) next.delete(path);
      else next.set(path, content);
      const side = (c: string | null, p: string): T.ChangeSide | undefined => (c === null ? undefined : { id: hashId(`blob:${p}:${c}`), mode: "file", size: c.length });
      const kind: T.ChangeKind = before === null ? "added" : content === null ? "deleted" : "modified";
      changes.push({ path, kind, old: side(before, path), new: side(content, path) });
    }
    const textOf = (path: string, side: "old" | "new") => (side === "old" ? snap.get(path) : next.get(path)) ?? null;
    changes = detectRenames(changes, textOf);
    changes.sort((a, b) => a.path.localeCompare(b.path));
    const patch = patchFor(changes, textOf);
    const agent = st.agent;
    const commit: T.Commit = {
      tree: hashId(`tree:${repo}:${i}:${[...next.keys()].join(",")}`),
      parents: parent ? [parent] : [],
      author: st.author ?? (agent ? bot(agent) : daniel),
      time_ms: st.at,
      message: st.msg,
      intent: st.intent ?? null,
      task: st.task ?? null,
      agent: agent ?? null,
      checkpoint: st.checkpoint ?? false,
      meta: st.meta ?? {},
    };
    // Hash the step index rather than the (NOW-relative) time so ids are stable across page loads.
    const id = hashId(`commit:${repo}:${parent}:${st.msg}:${i}`);
    out.push({ id, commit, changes, patch, snapshot: next, signature: signatureFor(st, agent?.id ?? commit.author.name.toLowerCase()) });
    snap = next;
    parent = id;
  });
  return out;
}

const T0 = NOW - 9 * DAY; // first commit 9 days ago
const at = (daysAgo: number, hour: number, minute = 0) => NOW - daysAgo * DAY - (23 - hour) * HOUR + minute * 60_000;

const shopMain: Step[] = [
  { msg: "Initial commit", intent: "Bootstrap the storefront service", task: "SHOP-1", at: T0, author: daniel, changes: { "README.md": F.readme0, "package.json": F.pkg, ".gitignore": F.gitignore } },
  { msg: "Add HTTP server skeleton", intent: "Serve a health endpoint so deploys can be verified", task: "SHOP-1", agent: agents.planner, at: T0 + 2 * HOUR, changes: { "src/index.ts": F.index, "src/api/server.ts": F.server0 } },
  { msg: "Add session model", intent: "Define the session and user shapes before implementing login", task: "SHOP-12", agent: agents.planner, at: T0 + 5 * HOUR, changes: { "src/auth/session.ts": F.session0 } },
  { msg: "Add login function", intent: "Allow users to sign in with email", task: "SHOP-12", agent: agents.coder1, at: at(8, 10), changes: { "src/auth/login.ts": F.login0 } },
  { msg: "Add in-memory session store for dev", intent: "Make login runnable without a database", task: "SHOP-12", agent: agents.coder1, at: at(8, 11, 20), changes: { "src/auth/store.ts": F.store } },
  { msg: "First login test", intent: "Lock down the unknown-user path", task: "SHOP-12", agent: agents.tester, at: at(8, 14), changes: { "tests/auth.test.ts": F.authTest0 } },
  { msg: "Wire POST /login", intent: "Expose login over HTTP", task: "SHOP-12", agent: agents.coder1, at: at(7, 9, 5), changes: { "src/api/server.ts": F.server1 } },
  { msg: "Add invoice model and total()", intent: "Start the billing module", task: "SHOP-20", agent: agents.coder2, at: at(7, 10), changes: { "src/billing/invoice.ts": F.invoice0 } },
  { msg: "Add country tax rates", intent: "Charge correct VAT per country", task: "SHOP-20", agent: agents.coder2, at: at(7, 11, 40), changes: { "src/billing/tax.ts": F.tax0 } },
  { msg: "Typed auth errors", intent: "Let the HTTP layer map auth failures to status codes", task: "SHOP-14", agent: agents.coder1, at: at(6, 9), changes: { "src/auth/errors.ts": F.errors } },
  { msg: "Lock accounts after 5 failed attempts\n\nNormalizes the email (trim + lowercase) before lookup so case differences do not create ghost users.", intent: "Slow down credential stuffing", task: "SHOP-14", agent: agents.coder1, at: at(6, 10, 30), changes: { "src/auth/login.ts": F.login1 } },
  { msg: "Test email normalization and AuthError", intent: "Cover the lockout change", task: "SHOP-14", agent: agents.tester, at: at(6, 12), changes: { "tests/auth.test.ts": F.authTest1 } },
  { msg: "Build invoices with tax", intent: "Produce complete invoices with subtotal, tax and total", task: "SHOP-20", agent: agents.coder2, at: at(5, 9, 15), changes: { "src/billing/invoice.ts": F.invoice1 } },
  { msg: "Billing tests", intent: "Verify German VAT math", task: "SHOP-20", agent: agents.tester, at: at(5, 11), changes: { "tests/billing.test.ts": F.billingTest.replace(/\n\n  it\("refuses empty invoices"[\s\S]*?\}\);\n/, "\n") } },
  { msg: "Add CI workflow", intent: "Run tests on every push", task: "SHOP-3", at: at(5, 15), author: daniel, changes: { ".github/workflows/ci.yml": F.ci } },
  { msg: "Add session rotation hook", intent: "Invalidate sessions on password change", task: "SHOP-31", agent: agents.coder1, at: at(4, 9), changes: { "src/auth/session.ts": F.session1 } },
  { msg: "Document architecture", intent: "Give agents a map before they touch routes", task: "SHOP-5", agent: agents.planner, at: at(4, 10), changes: { "docs/ARCHITECTURE.md": F.arch } },
  { msg: "Expand README", intent: "Explain the layout for new agents", task: "SHOP-5", agent: agents.planner, at: at(4, 10, 25), changes: { "README.md": F.readme1 } },
  { msg: "Add audit sink", intent: "Record security-relevant events for later shipping", task: "SHOP-40", agent: agents.coder2, at: at(3, 9), changes: { "src/audit.ts": F.audit } },
  { msg: "Audit login outcomes", intent: "Make lockouts observable", task: "SHOP-40", agent: agents.coder1, at: at(3, 11), changes: { "src/auth/login.ts": F.login2 }, sig: "untrusted" },
  { msg: "Merge pull request #1 from ws/coder-2\n\nAudit invoices and refuse empty ones", intent: "Land billing hardening", task: "SHOP-41", agent: agents.reviewer, at: at(2, 16), changes: { "src/billing/invoice.ts": F.invoice2, "tests/billing.test.ts": F.billingTest }, meta: { "agit.merge": { strategy: "merge", pull: 1 } }, sig: "trusted" },
  { msg: "Map AuthError to 401/423 and add POST /invoices", intent: "Expose billing over HTTP with proper auth status codes", task: "SHOP-42", agent: agents.coder1, at: at(1, 9, 30), changes: { "src/api/server.ts": F.server2 } },
  { msg: "Add logo asset", intent: "Branding for the storefront", task: "SHOP-50", at: at(1, 13), author: daniel, changes: { "assets/logo.png": "\u0089PNG\r\n\u001a\n\u0000\u0000\u0000\rIHDR\u0000\u0000" } },
  { msg: "Pin fastify to 5.3", intent: "Avoid the 5.4 schema regression", task: "SHOP-51", agent: agents.coder2, at: at(0, 8, 10), changes: { "package.json": F.pkg.replace('"^5.3.0"', '"5.3.0"') }, sig: "invalid" },
  { msg: "Move the architecture doc into docs/adr\n\nRenamed with a status header so future decisions can sit next to it.", intent: "Start an ADR log", task: "SHOP-5", agent: agents.planner, at: at(0, 8, 40), changes: { "docs/ARCHITECTURE.md": null, "docs/adr/0001-architecture.md": F.arch2 }, sig: "trusted" },
];

const shopMainCommits = buildHistory("acme/shop", shopMain);
const shopHead = shopMainCommits[shopMainCommits.length - 1];
const shopHeadSnap = shopHead.snapshot;
const mainAt = (n: number) => shopMainCommits[n];

// ws/coder-2: tax jurisdictions (open PR #2, mergeable)
const coder2Branch = buildHistory(
  "acme/shop",
  [
    { msg: "Add IT, ES, NL tax rates", intent: "Support the next three launch countries", task: "SHOP-60", agent: agents.coder2, at: at(0, 9, 5), changes: { "src/billing/tax.ts": F.tax1 } },
    { msg: "checkpoint", checkpoint: true, agent: agents.coder2, at: at(0, 9, 20), changes: { "src/billing/tax.ts": F.tax1 + "\n// TODO: CH\n" } },
    { msg: "Fail loudly on unknown jurisdictions", intent: "Never silently charge 0% tax", task: "SHOP-60", agent: agents.coder2, at: at(0, 10, 2), changes: { "src/billing/tax.ts": F.tax1, "tests/billing.test.ts": F.billingTest.replace("refuses empty invoices", "refuses empty invoices").concat(`\ndescribe("taxFor", () => {\n  it("throws on unknown jurisdictions", async () => {\n    const { taxFor } = await import("../src/billing/tax");\n    expect(() => taxFor("XX")).toThrow(/unknown tax jurisdiction/);\n  });\n});\n`) } },
  ],
  shopHeadSnap,
  shopHead.id,
);

// feature/tax-rates: conflicting edit of the same file from an older base (PR #3)
const featureBase = mainAt(19);
const featureBranch = buildHistory(
  "acme/shop",
  [{ msg: "Add Swiss VAT and jurisdiction listing", intent: "Prepare the CH launch", task: "SHOP-61", agent: agents.coder1, at: at(1, 17), sig: "untrusted", changes: { "src/billing/tax.ts": F.taxConflict, "src/billing/invoice.ts": F.invoice1.replace('import { taxFor } from "./tax";', 'import { taxFor, knownJurisdictions } from "./tax";\n\nexport const JURISDICTIONS = knownJurisdictions();') } }],
  featureBase.snapshot,
  featureBase.id,
);

// ws/coder-1: checkpoints and WIP (no PR)
const coder1Branch = buildHistory(
  "acme/shop",
  [
    { msg: "WIP: rate limiting middleware", intent: "Protect /login from brute force at the edge", task: "SHOP-70", agent: agents.coder1, at: at(0, 11, 45), changes: { "src/api/ratelimit.ts": `export function rateLimit(max: number, windowMs: number) {\n  const hits = new Map<string, number[]>();\n  return (key: string, now = Date.now()) => {\n    const arr = (hits.get(key) ?? []).filter((t) => now - t < windowMs);\n    arr.push(now);\n    hits.set(key, arr);\n    return arr.length <= max;\n  };\n}\n` } },
  ],
  shopHeadSnap,
  shopHead.id,
);

// ws/reviewer-1: closed PR #4 (docs tweak that was abandoned)
const reviewerBranch = buildHistory(
  "acme/shop",
  [{ msg: "Reword README running section", intent: "Clarify install steps", task: "SHOP-5", agent: agents.reviewer, at: at(2, 12), changes: { "README.md": F.readme1.replace("## Running", "## Getting started") } }],
  mainAt(19).snapshot,
  mainAt(19).id,
);

const docsMain = buildHistory("agit/docs-site", [
  { msg: "Initial site", intent: "Publish a landing page for the docs", task: "DOCS-1", at: at(3, 10), author: daniel, changes: { "README.md": F.docsReadme, "index.html": F.docsIndex, "styles.css": F.docsCss } },
  { msg: "Add intro page", intent: "Explain what agit is in one screen", task: "DOCS-2", agent: agents.planner, at: at(2, 9), changes: { "content/intro.md": F.docsIntro } },
  { msg: "Add build script", intent: "Turn markdown into HTML without a framework", task: "DOCS-3", agent: agents.coder2, at: at(1, 15, 30), changes: { "build.js": F.docsBuild } },
  { msg: "Tweak colours", intent: "Match the main brand palette", task: "DOCS-4", agent: agents.coder2, at: at(0, 7, 50), changes: { "styles.css": F.docsCss.replace("#0969da", "#1f6feb") } },
]);

// ---- repo state ----------------------------------------------------------------

interface RepoState {
  name: string;
  default_branch: string;
  created_ms: number;
  /** branch name -> linear commit list (oldest first) */
  branches: Map<string, BuiltCommit[]>;
  tags: T.Tag[];
  leases: T.Lease[];
  pulls: T.Pull[];
  notes: T.Note[];
  rules: T.TagRule[];
  policy: T.Policy;
  webhooks: T.Webhook[];
  events: T.Event[];
  keys: T.KeyEntry[];
}

function linear(...parts: BuiltCommit[][]) {
  return parts.flat();
}

const shop: RepoState = {
  name: "acme/shop",
  default_branch: "main",
  created_ms: T0,
  branches: new Map([
    ["main", shopMainCommits],
    ["ws/coder-2", linear(shopMainCommits, coder2Branch)],
    ["feature/tax-rates", linear(shopMainCommits.slice(0, 20), featureBranch)],
    ["ws/coder-1", linear(shopMainCommits, coder1Branch)],
    ["ws/reviewer-1", linear(shopMainCommits.slice(0, 20), reviewerBranch)],
  ]),
  tags: [
    { name: "v0.4.0", id: mainAt(17).id },
    { name: "v0.4.2", id: mainAt(21).id },
  ],
  leases: [],
  pulls: [],
  notes: [],
  rules: [
    { pattern: "src/auth/**", tags: ["area:auth"] },
    { pattern: "src/billing/**", tags: ["area:billing", "service:billing"] },
    { pattern: "src/api/**", tags: ["area:api"] },
    { pattern: "**/*.ts", tags: ["lang:ts"] },
  ],
  policy: {
    protected_branches: ["main", "release/*"],
    require_pull_request: true,
    require_approvals: 1,
    require_intent: true,
    require_agent: false,
    require_checks: false,
    block_on_blockers: true,
    allowed_merge_agents: ["reviewer-1", "planner-1"],
    allowed_push_agents: [],
    max_files_per_commit: 0,
    require_signed: false,
  },
  webhooks: [
    { id: "wh_3f9a1c2b", url: "https://ci.example.com/hooks/agit", kinds: ["commit", "pull.*"], active: true, has_secret: true, created_ms: at(5, 15, 10) },
    { id: "wh_8d04e7aa", url: "https://hooks.slack.example/T000/B000/agit", kinds: [], active: true, has_secret: false, created_ms: at(2, 9) },
  ],
  events: [],
  keys: [
    { agent: "planner-1", key_id: KEYS["planner-1"].key_id, public_key: KEYS["planner-1"].public_key, added_ms: at(6, 9), added_by: "root" },
    { agent: "reviewer-1", key_id: KEYS["reviewer-1"].key_id, public_key: KEYS["reviewer-1"].public_key, added_ms: at(6, 9, 5), added_by: "root" },
  ],
};

const docs: RepoState = {
  name: "agit/docs-site",
  default_branch: "main",
  created_ms: at(3, 10),
  branches: new Map([["main", docsMain]]),
  tags: [],
  leases: [],
  pulls: [],
  notes: [],
  rules: [{ pattern: "content/**", tags: ["area:content"] }],
  policy: { ...T_DEFAULT_POLICY },
  webhooks: [],
  events: [],
  keys: [],
};

const repos = new Map<string, RepoState>([
  [shop.name, shop],
  [docs.name, docs],
]);

function headOf(st: RepoState, branch: string): BuiltCommit | undefined {
  const list = st.branches.get(branch);
  return list?.[list.length - 1];
}

// leases
shop.leases.push(
  { id: "l_7f3c0d9e1a4b", holder: "coder-2", paths: ["src/billing/**"], mode: "exclusive", created_ms: NOW - 22 * 60_000, expires_ms: NOW + 18 * 60_000 + 20_000, note: "tax jurisdictions (SHOP-60)", workspace: "coder-2" },
  { id: "l_1c2d3e4f5a6b", holder: "coder-1", paths: ["src/api/ratelimit.ts", "src/api/server.ts"], mode: "exclusive", created_ms: NOW - 9 * 60_000, expires_ms: NOW + 51 * 60_000, note: "rate limiting (SHOP-70)", workspace: "coder-1" },
  { id: "l_9a8b7c6d5e4f", holder: "reviewer-1", paths: ["src/**"], mode: "shared", created_ms: NOW - 40 * 60_000, expires_ms: NOW + 4 * 60_000 + 30_000, note: "reviewing PR #2", workspace: "reviewer-1" },
  { id: "l_0011223344aa", holder: "tester-3", paths: ["tests"], mode: "exclusive", created_ms: NOW - 3 * HOUR, expires_ms: NOW - 20 * 60_000, note: "expired lease from a crashed run", workspace: "tester-3" },
);
docs.leases.push({ id: "l_doc001", holder: "coder-2", paths: ["styles.css"], mode: "exclusive", created_ms: NOW - 5 * 60_000, expires_ms: NOW + 25 * 60_000, note: "palette", workspace: "coder-2" });

// pulls
const mergedSrcHead = mainAt(20); // simulate: PR #1 came from ws/coder-2 and was merged at commit 20
shop.pulls.push(
  {
    number: 1,
    title: "Audit invoices and refuse empty ones",
    body: "Adds `audit()` calls to invoice creation and voiding, and rejects invoices with no line items.\n\n- [x] tests\n- [x] docs",
    intent: "Land billing hardening",
    task: "SHOP-41",
    source: "refs/heads/ws/coder-2",
    target: "refs/heads/main",
    source_id: mergedSrcHead.id,
    target_id: mainAt(19).id,
    base_id: mainAt(18).id,
    author: { id: "coder-2", model: "gpt-5-codex" },
    state: "merged",
    created_ms: at(2, 13),
    updated_ms: at(2, 16),
    merged_commit: mergedSrcHead.id,
    reviews: [
      { reviewer: "reviewer-1", decision: "approve", body: "Audit events look right and the empty-invoice guard matches the spec.", time_ms: at(2, 15, 30) },
    ],
    comments: [
      { id: 1, author: "reviewer-1", path: "src/billing/invoice.ts", line: 24, kind: "question", body: "Should the audit happen after the invoice is persisted rather than at build time?", resolved: true, time_ms: at(2, 14) },
    ],
    checks: [
      { name: "tests", status: "success", url: "https://ci.example.com/runs/811", time_ms: at(2, 13, 10) },
      { name: "lint", status: "success", url: null, time_ms: at(2, 13, 8) },
    ],
  },
  {
    number: 2,
    title: "Add IT/ES/NL tax rates and fail on unknown jurisdictions",
    body: "## Why\n\nWe launch in Italy, Spain and the Netherlands next sprint. `taxFor` currently returns `0` for anything unknown, which would under-charge silently.\n\n## What\n\n- new rates for IT, ES, NL\n- `taxFor` throws on unknown country codes\n- case-insensitive lookup\n\n## Risk\n\nCallers that pass lowercase codes now work; callers passing unknown codes now throw. `grep taxFor` shows only `buildInvoice`.",
    intent: "Support the next three launch countries without silent 0% tax",
    task: "SHOP-60",
    source: "refs/heads/ws/coder-2",
    target: "refs/heads/main",
    source_id: headOf(shop, "ws/coder-2")!.id,
    target_id: shopHead.id,
    base_id: shopHead.id,
    author: { id: "coder-2", model: "gpt-5-codex" },
    state: "open",
    created_ms: at(0, 10, 10),
    updated_ms: NOW - 12 * 60_000,
    merged_commit: null,
    reviews: [
      { reviewer: "reviewer-1", decision: "request_changes", body: "Throwing from `taxFor` is right, but the error should be typed so the API can return 422 instead of 500.", time_ms: at(0, 10, 40) },
      { reviewer: "tester-3", decision: "comment", body: "Ran the suite locally against the branch head: 7 passed.", time_ms: at(0, 10, 55) },
      { reviewer: "reviewer-1", decision: "approve", body: "Typed error can land in a follow-up; approving to unblock the launch branch.", time_ms: NOW - 12 * 60_000 },
    ],
    comments: [
      { id: 1, author: "reviewer-1", path: "src/billing/tax.ts", line: 13, kind: "blocker", body: "Use a `BillingError` here instead of a bare `Error` so the HTTP error handler can map it.", resolved: true, time_ms: at(0, 10, 38) },
      { id: 2, author: "reviewer-1", path: "src/billing/tax.ts", line: 12, kind: "suggestion", body: "`country.toUpperCase()` could move into `buildInvoice` so the rate table stays a pure lookup.", resolved: false, time_ms: at(0, 10, 39) },
      { id: 3, author: "coder-2", path: null, line: null, kind: "note", body: "Checkpoint `a1b2…` on this branch contained a stray TODO; squashed before pushing.", resolved: false, time_ms: at(0, 10, 50) },
      { id: 4, author: "planner-1", path: "tests/billing.test.ts", line: 17, kind: "question", body: "Do we want a table-driven test over all jurisdictions here?", resolved: false, time_ms: at(0, 11, 5) },
    ],
    checks: [
      { name: "tests", status: "success", url: "https://ci.example.com/runs/842", time_ms: at(0, 10, 20) },
      { name: "lint", status: "success", url: null, time_ms: at(0, 10, 18) },
      { name: "policy/required-fields", status: "success", url: null, time_ms: at(0, 10, 12) },
    ],
  },
  {
    number: 3,
    title: "Add Swiss VAT and jurisdiction listing",
    body: "Adds `CH` at 8.1% and a `knownJurisdictions()` helper for the admin UI.",
    intent: "Prepare the CH launch",
    task: "SHOP-61",
    source: "refs/heads/feature/tax-rates",
    target: "refs/heads/main",
    source_id: headOf(shop, "feature/tax-rates")!.id,
    target_id: shopHead.id,
    base_id: featureBase.id,
    author: { id: "coder-1", model: "claude-sonnet-4-5" },
    state: "open",
    created_ms: at(1, 17, 10),
    updated_ms: at(1, 18),
    merged_commit: null,
    reviews: [{ reviewer: "reviewer-1", decision: "comment", body: "Conflicts with main on `src/billing/invoice.ts` (the audit change from #1) and will conflict with #2 on `src/billing/tax.ts`; rebase once that lands.", time_ms: at(1, 18) }],
    comments: [{ id: 1, author: "reviewer-1", path: "src/billing/tax.ts", line: 5, kind: "blocker", body: "8.1% is correct from 2024, but the rate table in #2 restructures this file. Rebase first.", resolved: false, time_ms: at(1, 17, 50) }],
    checks: [
      { name: "tests", status: "failure", url: "https://ci.example.com/runs/839", time_ms: at(1, 17, 20) },
      { name: "lint", status: "pending", url: null, time_ms: at(1, 17, 12) },
    ],
  },
  {
    number: 4,
    title: "Reword README running section",
    body: "",
    intent: "Clarify install steps",
    task: "SHOP-5",
    source: "refs/heads/ws/reviewer-1",
    target: "refs/heads/main",
    source_id: headOf(shop, "ws/reviewer-1")!.id,
    target_id: mainAt(19).id,
    base_id: mainAt(19).id,
    author: { id: "reviewer-1", model: "claude-opus-4-1" },
    state: "closed",
    created_ms: at(2, 12, 5),
    updated_ms: at(2, 12, 40),
    merged_commit: null,
    reviews: [],
    comments: [{ id: 1, author: "planner-1", path: null, line: null, kind: "note", body: "Superseded by the README expansion in SHOP-5; closing.", resolved: true, time_ms: at(2, 12, 40) }],
    checks: [{ name: "tests", status: "success", url: null, time_ms: at(2, 12, 15) }],
  },
);

// notes
const note = (n: Partial<T.Note> & Pick<T.Note, "id" | "title" | "body" | "tags">): T.Note => ({
  paths: [],
  commits: [],
  pulls: [],
  author: "planner-1",
  confidence: 0.8,
  created_ms: at(4, 10),
  updated_ms: at(4, 10),
  superseded_by: null,
  valid_at: null,
  draft: false,
  stale: false,
  ...n,
});
shop.notes.push(
  note({ id: "n_auth_rotate", title: "Session tokens are rotated on password change", body: "Any code path that changes a password **must** call `SessionStore.rotate(userId)` afterwards. Forgetting this leaves old sessions valid.\n\nThe hook was added in SHOP-31. The in-memory store implements it; the Postgres store must do the same in one transaction with the password update.\n\n```ts\nawait store.setPassword(user.id, hash);\nawait store.rotate(user.id);\n```", tags: ["area:auth", "kind:gotcha", "lang:ts"], paths: ["src/auth/**"], commits: [mainAt(15).id], author: "coder-1", confidence: 0.95, created_ms: at(4, 9, 30), updated_ms: at(4, 9, 30), valid_at: mainAt(19).id }),
  note({ id: "n_auth_lockout", title: "Decision: lock accounts after 5 failed logins for 15 minutes", body: "We chose a fixed lockout over exponential backoff because the edge rate limiter (SHOP-70) handles the volumetric case. Lockout state lives on the user record (`failedAttempts`), reset on success.\n\nAlternatives considered: CAPTCHA (rejected: no human users at this stage), IP bans (rejected: agents share egress).", tags: ["area:auth", "kind:decision"], paths: ["src/auth/login.ts"], commits: [mainAt(10).id], pulls: [], author: "planner-1", confidence: 0.9, created_ms: at(6, 11), updated_ms: at(6, 11), valid_at: mainAt(10).id }),
  note({ id: "n_auth_errors", title: "How to add a new AuthError code", body: "1. Add the literal to `AuthCode` in `src/auth/errors.ts`.\n2. Map it to a status in `setErrorHandler` in `src/api/server.ts` (default is 401).\n3. Add a test in `tests/auth.test.ts` asserting `code`.\n\nDo not throw bare `Error` from anything under `src/auth`.", tags: ["area:auth", "area:api", "kind:howto"], paths: ["src/auth/errors.ts", "src/api/server.ts"], author: "coder-1", confidence: 0.85, created_ms: at(1, 10), updated_ms: at(1, 10), valid_at: mainAt(21).id }),
  note({ id: "n_billing_cents", title: "All money is integer cents", body: "Never use floating point for amounts. `unitCents`, `subtotalCents`, `taxCents`, `totalCents` are integers; tax is `Math.round(subtotal * rate)` once per invoice, not per line, which is what the accountant signed off on.", tags: ["area:billing", "kind:rule", "service:billing"], paths: ["src/billing/**"], author: "coder-2", confidence: 1, created_ms: at(5, 9, 40), updated_ms: at(5, 9, 40), valid_at: mainAt(12).id }),
  note({ id: "n_billing_tax_unknown", title: "Unknown jurisdictions must throw, not return 0", body: "Before PR #2 `taxFor` returned `0` for any unknown country. That silently under-charges. The new contract: throw. Callers validate country codes at the API boundary.\n\nSupersedes the earlier 'default to 0' decision.", tags: ["area:billing", "kind:decision", "service:billing"], paths: ["src/billing/tax.ts"], pulls: [2], author: "reviewer-1", confidence: 0.9, created_ms: at(0, 10, 45), updated_ms: at(0, 10, 45), valid_at: shopHead.id }),
  note({ id: "n_billing_tax_default0", title: "Decision: unknown countries default to 0% tax", body: "Early decision, superseded: unknown countries were charged no tax to keep the demo working.", tags: ["area:billing", "kind:decision"], paths: ["src/billing/tax.ts"], author: "coder-2", confidence: 0.3, created_ms: at(7, 11, 45), updated_ms: at(0, 10, 45), superseded_by: "n_billing_tax_unknown", valid_at: mainAt(8).id }),
  note({ id: "n_api_zod", title: "Validate every request body with zod at the route", body: "Routes parse their body with a zod schema before calling a domain function. Domain functions assume validated input and do not re-check shapes. Zod errors are returned as 400 by the default error handler.", tags: ["area:api", "kind:rule", "lang:ts"], paths: ["src/api/**"], author: "planner-1", confidence: 0.85 }),
  note({ id: "n_ci_fastify", title: "Gotcha: fastify 5.4 breaks schema compilation", body: "fastify 5.4.0 changed how `setErrorHandler` interacts with schema validation errors; our tests fail with `FST_ERR_VALIDATION`. Pinned to 5.3.0 in SHOP-51 until upstream fixes it (issue #5811).", tags: ["area:api", "kind:gotcha", "service:ci"], paths: ["package.json", "src/api/server.ts"], commits: [mainAt(23).id], author: "coder-2", confidence: 0.8, created_ms: at(0, 8, 15), updated_ms: at(0, 8, 15), valid_at: mainAt(23).id }),
  // Draft produced by the merge of PR #1 (see "Drafts from merges" in docs/API.md).
  note({ id: "n_draft_pr1", title: "Audit invoices and refuse empty ones", body: "Intent: Land billing hardening\nTask: SHOP-41\n\nAdds `audit()` calls to invoice creation and voiding, and rejects invoices with no line items.\n\nChanged files:\n- src/billing/invoice.ts (M)\n- tests/billing.test.ts (M)\n\nReviews:\n- reviewer-1: approve", tags: ["area:billing", "kind:change", "lang:ts", "service:billing"], paths: ["src/billing/**", "tests/**"], commits: [mainAt(20).id], pulls: [1], author: "reviewer-1", confidence: 0.4, created_ms: at(2, 16), updated_ms: at(2, 16), valid_at: mainAt(20).id, draft: true }),
);
docs.notes.push(note({ id: "n_docs_palette", title: "Palette source of truth is the main README", body: "Accent colour must match the badge colour used in the root README. Currently `#1f6feb`.", tags: ["area:content", "kind:rule"], paths: ["styles.css"], author: "coder-2", confidence: 0.7, valid_at: docsMain[0].id }));

// events: derive from history + leases + pulls, then sort by time
function seedEvents(st: RepoState) {
  const evs: Omit<T.Event, "seq">[] = [];
  evs.push({ time_ms: st.created_ms - 1000, kind: "repo.create", actor: "daniel", data: { name: st.name, default_branch: st.default_branch } });
  const seen = new Set<string>();
  for (const [branch, list] of st.branches) {
    for (const c of list) {
      if (seen.has(c.id)) continue;
      seen.add(c.id);
      const actor = c.commit.agent?.id ?? c.commit.author.name.toLowerCase();
      evs.push({ time_ms: c.commit.time_ms, kind: c.commit.checkpoint ? "checkpoint" : c.commit.parents.length > 1 || c.commit.message.startsWith("Merge") ? "merge" : "commit", workspace: branch.startsWith("ws/") ? branch.slice(3) : "main", actor, data: { id: c.id, branch, parents: c.commit.parents, summary: c.commit.message.split("\n")[0], intent: c.commit.intent, task: c.commit.task, author: c.commit.author.name } });
      evs.push({ time_ms: c.commit.time_ms + 5, kind: "ref.update", actor, data: { ref: `refs/heads/${branch}`, old: c.commit.parents[0] ?? null, new: c.id, message: null } });
    }
  }
  for (const l of st.leases) evs.push({ time_ms: l.created_ms, kind: "lease.claim", workspace: l.workspace, actor: l.holder, data: { id: l.id, paths: l.paths, mode: l.mode, expires_ms: l.expires_ms } });
  for (const p of st.pulls) {
    evs.push({ time_ms: p.created_ms, kind: "pull.open", actor: p.author.id, data: { number: p.number, title: p.title, source: p.source, target: p.target } });
    for (const r of p.reviews) evs.push({ time_ms: r.time_ms, kind: "pull.review", actor: r.reviewer, data: { number: p.number, decision: r.decision } });
    if (p.state === "merged") evs.push({ time_ms: p.updated_ms, kind: "pull.merge", actor: "reviewer-1", data: { number: p.number, merged_commit: p.merged_commit, strategy: "merge" } });
    if (p.state === "closed") evs.push({ time_ms: p.updated_ms, kind: "pull.close", actor: "planner-1", data: { number: p.number } });
  }
  for (const n of st.notes) evs.push(n.draft ? { time_ms: n.created_ms, kind: "note.draft", actor: n.author, data: { id: n.id, title: n.title, pull: n.pulls[0], commit: n.commits[0], tags: n.tags, paths: n.paths } } : { time_ms: n.created_ms, kind: "note.create", actor: n.author, data: { id: n.id, title: n.title, tags: n.tags } });
  evs.sort((a, b) => a.time_ms - b.time_ms);
  st.events = evs.map((e, i) => ({ seq: i + 1, ...e }));
}
repos.forEach(seedEvents);

function pushEvent(st: RepoState, e: Omit<T.Event, "seq" | "time_ms"> & { time_ms?: number }): T.Event {
  const ev: T.Event = { seq: (st.events[st.events.length - 1]?.seq ?? 0) + 1, time_ms: e.time_ms ?? Date.now(), kind: e.kind, workspace: e.workspace, actor: e.actor, data: e.data };
  st.events.push(ev);
  streamListeners.get(st.name)?.forEach((l) => l(ev));
  return ev;
}

// simulated live activity
const streamListeners = new Map<string, Set<(e: T.Event) => void>>();
let simTimer: ReturnType<typeof setInterval> | null = null;
function ensureSimulation() {
  if (simTimer) return;
  const pool = [agents.coder1, agents.coder2, agents.tester, agents.reviewer];
  const intents = ["Tighten input validation", "Reduce allocations in hot path", "Add missing test for lockout", "Refactor route registration", "Document tax table", "Retry on ref_conflict"];
  let n = 0;
  simTimer = setInterval(() => {
    const st = shop;
    n++;
    const a = pool[n % pool.length];
    const roll = n % 5;
    if (roll === 0) pushEvent(st, { kind: "checkpoint", workspace: a.id, actor: a.id, data: { id: hashId(`sim:${n}`), branch: `ws/${a.id}` } });
    else if (roll === 1) {
      const l = st.leases.find((x) => x.holder === a.id && (x.expires_ms ?? 0) > Date.now());
      if (l) {
        l.expires_ms = Date.now() + 30 * 60_000;
        pushEvent(st, { kind: "lease.renew", workspace: l.workspace, actor: a.id, data: { id: l.id, expires_ms: l.expires_ms } });
      } else pushEvent(st, { kind: "head.switch", workspace: a.id, actor: a.id, data: { from: "main", to: `ws/${a.id}` } });
    } else if (roll === 2) pushEvent(st, { kind: "commit", workspace: a.id, actor: a.id, data: { id: hashId(`sim:${n}`), branch: `ws/${a.id}`, parents: [hashId(`sim:${n - 1}`)], summary: intents[n % intents.length], intent: intents[n % intents.length], task: `SHOP-${60 + (n % 12)}`, author: a.id } });
    else if (roll === 3) pushEvent(st, { kind: "ref.update", actor: a.id, data: { ref: `refs/heads/ws/${a.id}`, old: hashId(`sim:${n - 1}`), new: hashId(`sim:${n}`), message: null } });
    else pushEvent(st, { kind: "lease.check", actor: a.id, data: { paths: ["src/api/server.ts"], allowed: a.id === "coder-1" } });
  }, 4500);
}

// ---- lookups -----------------------------------------------------------------

function repoOf(name: string): RepoState {
  return repos.get(name) ?? notFound(`repository ${name}`);
}

function allCommits(st: RepoState): Map<string, BuiltCommit> {
  const m = new Map<string, BuiltCommit>();
  for (const list of st.branches.values()) for (const c of list) m.set(c.id, c);
  return m;
}

/** Resolve a rev (branch, tag, full or short id, refs/heads/x, HEAD) to a commit + its linear history. */
function resolve(st: RepoState, rev: string): { commit: BuiltCommit; history: BuiltCommit[] } {
  // `<base>~N` / `<base>^` walk first parents (mock history is linear per branch).
  const nav = /^(.*?)((?:[~^]\d*)+)$/.exec(rev);
  if (nav) {
    const base = resolve(st, nav[1]);
    let back = 0;
    for (const m of nav[2].matchAll(/[~^](\d*)/g)) back += m[1] ? parseInt(m[1], 10) : 1;
    const idx = base.history.length - 1 - back;
    if (idx < 0) throw new ApiError(404, "object_not_found", `revision ${rev} not found`, false);
    return { commit: base.history[idx], history: base.history.slice(0, idx + 1) };
  }
  let r = rev;
  if (r === "HEAD" || r === "") r = st.default_branch;
  if (r.startsWith("refs/heads/")) r = r.slice("refs/heads/".length);
  if (r.startsWith("refs/tags/")) r = r.slice("refs/tags/".length);
  const list = st.branches.get(r);
  if (list) return { commit: list[list.length - 1], history: list };
  const tag = st.tags.find((t) => t.name === r);
  if (tag) r = tag.id;
  for (const hist of st.branches.values()) {
    const idx = hist.findIndex((c) => c.id === r || (r.length >= 6 && c.id.startsWith(r)));
    if (idx >= 0) return { commit: hist[idx], history: hist.slice(0, idx + 1) };
  }
  throw new ApiError(404, "object_not_found", `revision ${rev} not found`, false);
}

function listItem(c: BuiltCommit): T.CommitListItem {
  return { id: c.id, ...c.commit };
}

function diffBetween(st: RepoState, from: string, to: string): T.Diff {
  const a = resolve(st, from).commit.snapshot;
  const b = resolve(st, to).commit.snapshot;
  const paths = [...new Set([...a.keys(), ...b.keys()])].sort();
  let changes: T.Change[] = [];
  for (const p of paths) {
    const o = a.get(p) ?? null, n = b.get(p) ?? null;
    if (o === n) continue;
    const kind: T.ChangeKind = o === null ? "added" : n === null ? "deleted" : "modified";
    const side = (c: string | null): T.ChangeSide | undefined => (c === null ? undefined : { id: hashId(`blob:${p}:${c}`), mode: "file", size: c.length });
    changes.push({ path: p, kind, old: side(o), new: side(n) });
  }
  const textOf = (p: string, side: "old" | "new") => (side === "old" ? a.get(p) : b.get(p)) ?? null;
  changes = detectRenames(changes, textOf);
  const stats: T.FileStat[] = [];
  let patch = "";
  for (const c of changes) {
    const d = patchFor([c], textOf);
    patch += d;
    let added = 0, removed = 0;
    for (const line of d.split("\n")) {
      if (line.startsWith("+") && !line.startsWith("+++")) added++;
      else if (line.startsWith("-") && !line.startsWith("---")) removed++;
    }
    stats.push({ path: c.path, added, removed });
  }
  return { changes, patch, stats };
}

/** Paths covered by the note that differ between `valid_at` and the default branch head (stale = non-empty). */
const staleCache = new Map<string, T.Change[]>();
function changedPaths(st: RepoState, n: T.Note): string[] {
  if (!n.valid_at || !n.paths.length) return [];
  const head = headOf(st, st.default_branch);
  if (!head || head.id === n.valid_at) return [];
  const key = `${st.name}\0${n.valid_at}\0${head.id}`;
  let changes = staleCache.get(key);
  if (!changes) {
    try {
      changes = diffBetween(st, n.valid_at, head.id).changes;
    } catch {
      changes = [];
    }
    staleCache.set(key, changes);
  }
  const out = new Set<string>();
  for (const c of changes) {
    for (const p of n.paths) {
      if (patternCovers(p, c.path)) out.add(c.path);
      if (c.from && patternCovers(p, c.from)) out.add(c.from);
    }
  }
  return [...out].sort();
}
const withStale = (st: RepoState, n: T.Note): T.Note => ({ ...structuredClone(n), stale: changedPaths(st, n).length > 0 });

function summary(st: RepoState, n: T.Note, score?: number): T.NoteSummary {
  const { body, created_ms: _c, commits: _k, pulls: _p, ...rest } = n;
  const firstLine = body.split("\n").find((l) => l.trim() && !l.startsWith("#") && !l.startsWith("```")) ?? "";
  const out: T.NoteSummary = { ...rest, draft: !!n.draft, stale: changedPaths(st, n).length > 0, excerpt: firstLine.slice(0, 200) };
  if (score !== undefined) out.score = score;
  return out;
}

/** BM25-ish: weighted term hits (title 3, tags 2, body and paths 1) with prefix matches at half weight. */
function scoreNote(n: T.Note, q: string): number {
  const terms = q.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  if (!terms.length) return 0;
  const fields: [string, number][] = [[n.title, 3], [n.tags.join(" "), 2], [n.body, 1], [n.paths.join(" "), 1]];
  let score = 0;
  for (const t of terms) {
    for (const [text, w] of fields) {
      const words = text.toLowerCase().split(/[^\p{L}\p{N}]+/u);
      const exact = words.filter((x) => x === t).length;
      const prefix = t.length >= 3 ? words.filter((x) => x !== t && x.startsWith(t)).length : 0;
      score += w * (exact + prefix * 0.5);
    }
  }
  return Math.round(score * 100) / 100;
}

function draftFromMerge(st: RepoState, pr: T.Pull, merged: string, changes: T.Change[], actor: string): T.Note {
  const now = Date.now();
  const dirs = [...new Set(changes.map((c) => (c.path.includes("/") ? c.path.slice(0, c.path.lastIndexOf("/")) + "/**" : c.path)))];
  const tags = new Set(["kind:change"]);
  for (const c of changes) for (const r of st.rules) if (patternCovers(r.pattern, c.path)) r.tags.forEach((t) => tags.add(t));
  const latest = new Map<string, string>();
  for (const r of pr.reviews) latest.set(r.reviewer, r.decision);
  const letter: Record<T.ChangeKind, string> = { added: "A", modified: "M", deleted: "D", type_changed: "T", renamed: "R" };
  const body = [pr.intent ? `Intent: ${pr.intent}` : null, pr.task ? `Task: ${pr.task}` : null, "", pr.body, "", "Changed files:", ...changes.map((c) => `- ${c.path} (${letter[c.kind]})`), "", "Reviews:", ...[...latest].map(([r, d]) => `- ${r}: ${d}`)].filter((l) => l !== null).join("\n");
  return { id: "n_" + hashId(`draft:${pr.number}:${merged}`).slice(0, 10), title: pr.title, body, tags: [...tags].sort(), paths: dirs, commits: [merged], pulls: [pr.number], author: actor, confidence: 0.4, created_ms: now, updated_ms: now, superseded_by: null, valid_at: merged, draft: true, stale: false };
}

function actingAgent() {
  return loadSettings().agent || "web-ui";
}

// ---- auth fixtures -------------------------------------------------------------

/** Well-known mock tokens: `root-secret` (root), `ci-secret` (write on acme/shop as ci-bot), `reader-secret` (read everywhere). */
const tokenStore: { info: T.TokenInfo; secret: string }[] = [
  { secret: "ci-secret", info: { id: "tok_" + hashId("tok:ci").slice(0, 12), name: "ci-deploy", agent: "ci-bot", scopes: [{ repo: "acme/shop", access: "write" }], created_ms: at(5, 15, 20), expires_ms: NOW + 25 * DAY, created_by: "root", last_used_ms: NOW - 40 * 60_000 } },
  { secret: "reader-secret", info: { id: "tok_" + hashId("tok:reader").slice(0, 12), name: "dashboard-reader", scopes: [{ repo: "*", access: "read" }], created_ms: at(3, 9), expires_ms: null, created_by: "root", last_used_ms: null } },
  { secret: hashId("tok:expired").slice(0, 40), info: { id: "tok_" + hashId("tok:old").slice(0, 12), name: "old-laptop", agent: "coder-1", scopes: [{ repo: "acme/shop", access: "admin" }, { repo: "agit/docs-site", access: "read" }], created_ms: at(8, 12), expires_ms: NOW - 2 * DAY, created_by: "root", last_used_ms: at(3, 12) } },
];

function principal(): T.WhoAmI {
  const token = loadSettings().token;
  if (!token) return { principal: "anonymous", root: true, scopes: [{ repo: "*", access: "admin" }] }; // open mode
  if (token === "root-secret") return { principal: "root", root: true, scopes: [{ repo: "*", access: "admin" }] };
  const t = tokenStore.find((x) => x.secret === token);
  if (!t) throw new ApiError(403, "forbidden", "unknown or revoked token", false);
  if (t.info.expires_ms && t.info.expires_ms < Date.now()) throw new ApiError(403, "forbidden", `token ${t.info.name} expired`, false);
  t.info.last_used_ms = Date.now();
  return { principal: t.info.id, agent: t.info.agent ?? undefined, root: false, scopes: t.info.scopes };
}
const RANK: Record<T.Access, number> = { read: 0, write: 1, admin: 2 };
function holds(who: T.WhoAmI, repo: string, access: T.Access) {
  return who.root || who.scopes.some((s) => (s.repo === "*" || s.repo === repo) && RANK[s.access] >= RANK[access]);
}
/** Mock enforcement of the access rules in docs/API.md (admin for policy/keys/webhooks/rules/tokens, write for other mutations). */
function require(repo: string, access: T.Access) {
  const who = principal();
  if (!holds(who, repo, access)) {
    const err = new ApiError(403, "forbidden", `${access} access to ${repo === "*" ? "the server" : repo} required (you are ${who.agent ?? who.principal})`, false);
    notifyAuthDenied(err); // the HTTP layer does this for live responses
    throw err;
  }
  return who;
}

// ---- Api implementation ---------------------------------------------------------

export const mockApi: Api = {
  async health() {
    await sleep(20);
    return { status: "ok", version: "0.1.0-mock" };
  },

  async whoami() {
    await sleep(60);
    return principal();
  },
  async tokens() {
    await latency();
    require("*", "admin");
    return tokenStore.map((t) => structuredClone(t.info));
  },
  async createToken(body) {
    await latency();
    const who = require("*", "admin");
    if (!body.name?.trim()) throw new ApiError(400, "invalid", "name is required", false);
    if (!body.scopes?.length) throw new ApiError(400, "invalid", "at least one scope is required", false);
    for (const s of body.scopes) if (!holds(who, s.repo, s.access) || (s.repo === "*" && !who.root)) throw new ApiError(403, "forbidden", `cannot grant ${s.access} on ${s.repo}: you do not hold it`, false);
    const now = Date.now();
    const secret = "agit_" + hashId(`secret:${now}:${body.name}`).slice(0, 40);
    const info: T.TokenInfo = { id: "tok_" + hashId(`tok:${now}:${body.name}`).slice(0, 12), name: body.name.trim(), agent: body.agent?.trim() || undefined, scopes: body.scopes, created_ms: now, expires_ms: body.ttl_secs ? now + body.ttl_secs * 1000 : null, created_by: who.agent ?? who.principal, last_used_ms: null };
    tokenStore.push({ secret, info });
    return { token: secret, ...structuredClone(info) };
  },
  async deleteToken(id) {
    await latency();
    require("*", "admin");
    const idx = tokenStore.findIndex((t) => t.info.id === id);
    if (idx < 0) notFound(`token ${id}`);
    tokenStore.splice(idx, 1);
    return { deleted: id };
  },

  async repos() {
    await latency();
    return [...repos.values()].map((st) => ({ name: st.name, default_branch: st.default_branch, head: headOf(st, st.default_branch)?.id ?? null, created_ms: st.created_ms }));
  },
  async createRepo(body) {
    await latency();
    require("*", "admin");
    if (repos.has(body.name)) throw new ApiError(409, "already_exists", `repository ${body.name} already exists`, false);
    const st: RepoState = { name: body.name, default_branch: body.default_branch ?? "main", created_ms: Date.now(), branches: new Map(), tags: [], leases: [], pulls: [], notes: [], rules: [], policy: { ...T_DEFAULT_POLICY }, webhooks: [], events: [], keys: [] };
    repos.set(st.name, st);
    pushEvent(st, { kind: "repo.create", actor: actingAgent(), data: { name: st.name } });
    return { name: st.name, default_branch: st.default_branch, head: null, created_ms: st.created_ms };
  },
  async repo(name) {
    await latency();
    const st = repoOf(name);
    return { name: st.name, default_branch: st.default_branch, head: headOf(st, st.default_branch)?.id ?? null, created_ms: st.created_ms, branches: st.branches.size, tags: st.tags.length, objects: allCommits(st).size * 3 };
  },
  async deleteRepo(name) {
    await latency();
    repoOf(name);
    require(name, "admin");
    repos.delete(name);
    return { deleted: true };
  },

  async refs(name, prefix = "") {
    await latency();
    const st = repoOf(name);
    const out: T.Ref[] = [];
    for (const [b, list] of st.branches) out.push({ name: `refs/heads/${b}`, id: list[list.length - 1].id });
    for (const t of st.tags) out.push({ name: `refs/tags/${t.name}`, id: t.id });
    return out.filter((r) => r.name.startsWith(prefix)).sort((a, b) => a.name.localeCompare(b.name));
  },
  async updateRefs() {
    throw new ApiError(403, "forbidden", "ref updates are disabled in mock mode", false);
  },

  async objectJson(name, id) {
    await latency();
    const st = repoOf(name);
    const c = allCommits(st).get(id);
    if (c) return { id, kind: "commit", size: JSON.stringify(c.commit).length, commit: c.commit };
    for (const bc of allCommits(st).values()) for (const [p, text] of bc.snapshot) if (hashId(`blob:${p}:${text}`) === id) return { id, kind: "blob", size: text.length, text };
    return notFound(`object ${id}`);
  },

  async commits(name, q = {}) {
    await latency();
    const st = repoOf(name);
    const { history } = resolve(st, q.rev ?? st.default_branch);
    let list = [...history].reverse();
    if (q.agent) list = list.filter((c) => c.commit.agent?.id === q.agent);
    if (q.task) list = list.filter((c) => c.commit.task === q.task);
    if (q.path) list = list.filter((c) => c.changes.some((ch) => patternCovers(q.path!, ch.path)));
    return list.slice(0, q.limit ?? 50).map(listItem);
  },
  async commit(name, rev) {
    await latency();
    const st = repoOf(name);
    const { commit } = resolve(st, rev);
    const linking = st.notes.filter((n) => n.commits.includes(commit.id));
    const covering = st.notes.filter((n) => !n.draft && !linking.includes(n) && commit.changes.some((c) => n.paths.some((p) => patternCovers(p, c.path))));
    const notes = [...linking, ...covering].slice(0, 10).map((n) => summary(st, n));
    return { id: commit.id, commit: commit.commit, changes: commit.changes, patch: commit.patch, signature: commit.signature, notes };
  },
  async blame(name, rev, path, maxCommits = 500) {
    await latency();
    const st = repoOf(name);
    const head = resolve(st, rev).commit;
    // Same algorithm as the server: walk the file's history and attribute lines newest first.
    const r = await computeBlame(mockApi, name, head.id, path);
    if (r.binary) throw new ApiError(400, "invalid", `${path} is binary`, false);
    const commits: Record<string, T.BlameCommit> = {};
    for (const c of r.commits) commits[c.id] = { id: c.id, author: c.author.name, agent: c.agent?.id ?? undefined, time_ms: c.time_ms, summary: c.message.split("\n")[0], intent: c.intent ?? undefined };
    const used = new Set(r.lines.map((l) => (l.commit >= 0 ? r.commits[l.commit].id : null)).filter(Boolean) as string[]);
    for (const id of Object.keys(commits)) if (!used.has(id)) delete commits[id];
    return { path, rev: head.id, lines: r.lines.map((l) => ({ line: l.no, commit: l.commit >= 0 ? r.commits[l.commit].id : r.commits[r.commits.length - 1].id, text: l.text })), commits, truncated: r.truncated || r.commits.length >= maxCommits };
  },
  async tree(name, rev, path = "") {
    await latency();
    const st = repoOf(name);
    const snap = resolve(st, rev).commit.snapshot;
    const p = path.replace(/^\/+|\/+$/g, "");
    if (p && snap.has(p)) {
      const text = snap.get(p)!;
      const binary = /[\u0000-\u0008]/.test(text) || p.endsWith(".png");
      return { kind: "blob", path: p, id: hashId(`blob:${p}:${text}`), size: text.length, binary, text: binary ? undefined : text };
    }
    const prefix = p ? p + "/" : "";
    const entries = new Map<string, T.TreeEntry>();
    for (const [fp, text] of snap) {
      if (!fp.startsWith(prefix)) continue;
      const rest = fp.slice(prefix.length);
      const slash = rest.indexOf("/");
      if (slash < 0) entries.set(rest, { name: rest, mode: rest.endsWith(".sh") ? "executable" : "file", id: hashId(`blob:${fp}:${text}`), size: text.length });
      else {
        const dir = rest.slice(0, slash);
        const cur = entries.get(dir);
        entries.set(dir, { name: dir, mode: "dir", id: hashId(`tree:${prefix}${dir}:${rev}`), size: (cur?.size ?? 0) + text.length });
      }
    }
    if (entries.size === 0 && p) notFound(`path ${p}`);
    const list = [...entries.values()].sort((a, b) => (a.mode === "dir") === (b.mode === "dir") ? a.name.localeCompare(b.name) : a.mode === "dir" ? -1 : 1);
    return { kind: "tree", path: p, id: hashId(`tree:${p}:${resolve(st, rev).commit.commit.tree}`), entries: list };
  },
  async diff(name, from, to) {
    await latency();
    return diffBetween(repoOf(name), from, to);
  },
  async branches(name) {
    await latency();
    const st = repoOf(name);
    const main = st.branches.get(st.default_branch) ?? [];
    const mainIds = new Set(main.map((c) => c.id));
    return [...st.branches.entries()].map(([b, list]) => {
      const ahead = list.filter((c) => !mainIds.has(c.id)).length;
      const ids = new Set(list.map((c) => c.id));
      const behind = main.filter((c) => !ids.has(c.id)).length;
      return { name: b, id: list[list.length - 1].id, ahead, behind };
    }).sort((a, b) => (a.name === st.default_branch ? -1 : b.name === st.default_branch ? 1 : a.name.localeCompare(b.name)));
  },
  async tags(name) {
    await latency();
    return [...repoOf(name).tags];
  },

  async events(name, since = 0, limit = 100) {
    await sleep(60);
    const st = repoOf(name);
    const evs = st.events.filter((e) => e.seq > since);
    return evs.length > limit ? evs.slice(evs.length - limit) : evs;
  },
  streamEvents(name, since, onEvent, onStatus?: (s: StreamStatus) => void) {
    const st = repoOf(name);
    ensureSimulation();
    onStatus?.("connecting");
    let closed = false;
    const timer = setTimeout(() => {
      if (closed) return;
      st.events.filter((e) => e.seq > since).forEach(onEvent);
      onStatus?.("sse");
    }, 150);
    let set = streamListeners.get(name);
    if (!set) streamListeners.set(name, (set = new Set()));
    const l = (e: T.Event) => onEvent(e);
    set.add(l);
    return () => {
      closed = true;
      clearTimeout(timer);
      set!.delete(l);
    };
  },

  async leases(name) {
    await latency();
    return [...repoOf(name).leases];
  },
  async createLease(name, body) {
    await latency();
    const st = repoOf(name);
    const now = Date.now();
    const conflict = st.leases.find((l) => (l.expires_ms ?? Infinity) > now && l.holder !== body.holder && (l.mode === "exclusive" || body.mode === "exclusive") && l.paths.some((lp) => body.paths.some((bp) => patternCovers(lp, bp) || patternCovers(bp, lp))));
    if (conflict) throw new ApiError(409, "lease_conflict", `paths overlap lease ${conflict.id} held by ${conflict.holder}`, false, { lease: conflict });
    const lease: T.Lease = { id: "l_" + hashId(`lease:${now}:${body.holder}`).slice(0, 12), holder: body.holder, paths: body.paths, mode: body.mode, created_ms: now, expires_ms: now + body.ttl_secs * 1000, note: body.note ?? null, workspace: null };
    st.leases.push(lease);
    pushEvent(st, { kind: "lease.claim", actor: body.holder, data: { id: lease.id, paths: lease.paths, mode: lease.mode, expires_ms: lease.expires_ms } });
    return lease;
  },
  async renewLease(name, id, ttl_secs) {
    await latency();
    const st = repoOf(name);
    const l = st.leases.find((x) => x.id === id) ?? notFound(`lease ${id}`);
    l.expires_ms = Date.now() + ttl_secs * 1000;
    pushEvent(st, { kind: "lease.renew", actor: actingAgent(), data: { id, expires_ms: l.expires_ms } });
    return l;
  },
  async releaseLease(name, id) {
    await latency();
    const st = repoOf(name);
    const idx = st.leases.findIndex((x) => x.id === id);
    if (idx < 0) notFound(`lease ${id}`);
    const [l] = st.leases.splice(idx, 1);
    pushEvent(st, { kind: "lease.release", actor: actingAgent(), data: { id, holder: l.holder, paths: l.paths } });
    return { released: true };
  },
  async checkLease(name, holder, paths) {
    await latency();
    const st = repoOf(name);
    const now = Date.now();
    const conflict = st.leases.find((l) => (l.expires_ms ?? Infinity) > now && l.holder !== holder && l.mode === "exclusive" && l.paths.some((lp) => paths.some((p) => patternCovers(lp, p))));
    if (conflict) throw new ApiError(409, "lease_conflict", `${paths.join(", ")} covered by lease ${conflict.id} held by ${conflict.holder}`, false);
    return { allowed: true };
  },

  async pulls(name, state) {
    await latency();
    const st = repoOf(name);
    const list = state && state !== "all" ? st.pulls.filter((p) => p.state === state) : st.pulls;
    return [...list].sort((a, b) => b.updated_ms - a.updated_ms).map((p) => structuredClone(p));
  },
  async createPull(name, body) {
    await latency();
    const st = repoOf(name);
    if (!body.title?.trim()) throw new ApiError(400, "invalid", "title is required", false);
    const target = body.target ?? `refs/heads/${st.default_branch}`;
    const srcRes = resolve(st, body.source);
    const src = srcRes.commit;
    const tgtRes = resolve(st, target);
    const tgt = tgtRes.commit;
    const tgtIds = new Set(tgtRes.history.map((c) => c.id));
    const mergeBase = [...srcRes.history].reverse().find((c) => tgtIds.has(c.id)) ?? tgt;
    const now = Date.now();
    const pr: T.Pull = {
      number: Math.max(0, ...st.pulls.map((p) => p.number)) + 1,
      title: body.title,
      body: body.body ?? "",
      intent: body.intent ?? null,
      task: body.task ?? null,
      source: body.source.startsWith("refs/") ? body.source : `refs/heads/${body.source}`,
      target: target.startsWith("refs/") ? target : `refs/heads/${target}`,
      source_id: src.id,
      target_id: tgt.id,
      base_id: mergeBase.id,
      author: { id: actingAgent(), model: null },
      state: "open",
      created_ms: now,
      updated_ms: now,
      merged_commit: null,
      reviews: [],
      comments: [],
      checks: [],
    };
    st.pulls.push(pr);
    pushEvent(st, { kind: "pull.open", actor: pr.author.id, data: { number: pr.number, title: pr.title, source: pr.source, target: pr.target } });
    return structuredClone(pr);
  },
  async pull(name, n) {
    await latency();
    const st = repoOf(name);
    const pr = st.pulls.find((p) => p.number === n) ?? notFound(`pull request #${n}`);
    const srcHist = resolve(st, pr.source_id).history;
    const baseIdx = srcHist.findIndex((c) => c.id === pr.base_id);
    const commits = srcHist.slice(baseIdx + 1).reverse().map(listItem);
    const d = diffBetween(st, pr.base_id, pr.source_id);
    // conflicts: files changed on both sides since base
    const tgtDiff = pr.state === "open" ? diffBetween(st, pr.base_id, pr.target) : { changes: [] as T.Change[] };
    const theirs = new Set(tgtDiff.changes.map((c) => c.path));
    const conflicts = d.changes.map((c) => c.path).filter((p) => theirs.has(p));
    const blockers = pr.comments.some((c) => c.kind === "blocker" && !c.resolved);
    return { pr: structuredClone(pr), commits, changes: d.changes, mergeable: pr.state === "open" && conflicts.length === 0 && !blockers, conflicts };
  },
  async pullDiff(name, n) {
    await latency();
    const st = repoOf(name);
    const pr = st.pulls.find((p) => p.number === n) ?? notFound(`pull request #${n}`);
    return diffBetween(st, pr.base_id, pr.source_id);
  },
  async addPullComment(name, n, body) {
    await latency();
    const st = repoOf(name);
    const pr = st.pulls.find((p) => p.number === n) ?? notFound(`pull request #${n}`);
    const c: T.PullComment = { id: Math.max(0, ...pr.comments.map((x) => x.id)) + 1, author: actingAgent(), path: body.path ?? null, line: body.line ?? null, kind: body.kind, body: body.body, resolved: false, time_ms: Date.now() };
    pr.comments.push(c);
    pr.updated_ms = c.time_ms;
    pushEvent(st, { kind: "pull.comment", actor: c.author, data: { number: n, comment: c.id, path: c.path, kind: c.kind } });
    return { ...c };
  },
  async resolvePullComment(name, n, id) {
    await latency();
    const st = repoOf(name);
    const pr = st.pulls.find((p) => p.number === n) ?? notFound(`pull request #${n}`);
    const c = pr.comments.find((x) => x.id === id) ?? notFound(`comment ${id}`);
    c.resolved = true;
    pr.updated_ms = Date.now();
    return { ...c };
  },
  async reviewPull(name, n, body) {
    await latency();
    const st = repoOf(name);
    const pr = st.pulls.find((p) => p.number === n) ?? notFound(`pull request #${n}`);
    if (pr.state !== "open") throw new ApiError(409, "invalid", `pull request #${n} is ${pr.state}`, false);
    const review: T.Review = { reviewer: actingAgent(), decision: body.decision, body: body.body, time_ms: Date.now() };
    pr.reviews.push(review);
    pr.updated_ms = review.time_ms;
    pushEvent(st, { kind: "pull.review", actor: review.reviewer, data: { number: n, decision: body.decision } });
    return structuredClone(pr);
  },
  async addPullCheck(name, n, body) {
    await latency();
    const st = repoOf(name);
    const pr = st.pulls.find((p) => p.number === n) ?? notFound(`pull request #${n}`);
    const existing = pr.checks.find((c) => c.name === body.name);
    const check: T.Check = { name: body.name, status: body.status, url: body.url ?? null, time_ms: Date.now() };
    if (existing) Object.assign(existing, check);
    else pr.checks.push(check);
    return structuredClone(pr);
  },
  async mergePull(name, n, body) {
    await latency();
    const st = repoOf(name);
    const detail = await mockApi.pull(name, n);
    const pr = st.pulls.find((p) => p.number === n)!;
    if (pr.state !== "open") throw new ApiError(409, "invalid", `pull request #${n} is ${pr.state}`, false);
    if (detail.conflicts.length) throw new ApiError(409, "merge_conflicts", `conflicts in ${detail.conflicts.join(", ")}`, false, { conflicts: detail.conflicts });
    if (!detail.mergeable) throw new ApiError(409, "policy_violation", "open blocker comments prevent merging", false);
    const targetBranch = pr.target.replace("refs/heads/", "");
    const tgt = st.branches.get(targetBranch)!;
    const srcHead = resolve(st, pr.source).commit;
    let mergedId: string;
    if (body.strategy === "ff" || tgt[tgt.length - 1].id === pr.base_id) {
      const srcHist = resolve(st, pr.source).history;
      const baseIdx = srcHist.findIndex((c) => c.id === pr.base_id);
      const extra = srcHist.slice(baseIdx + 1).filter((c) => !c.commit.checkpoint);
      if (body.strategy === "ff" && tgt[tgt.length - 1].id !== pr.base_id) throw new ApiError(409, "merge_conflicts", "target moved; fast-forward not possible", false);
      if (body.strategy === "merge") {
        const mergeCommit: BuiltCommit = { id: hashId(`merge:${n}:${Date.now()}`), commit: { tree: srcHead.commit.tree, parents: [tgt[tgt.length - 1].id, srcHead.id], author: { name: actingAgent(), email: `${actingAgent()}@agents.local` }, time_ms: Date.now(), message: body.message ?? `Merge pull request #${n} from ${pr.source.replace("refs/heads/", "")}\n\n${pr.title}`, intent: pr.intent, task: pr.task, agent: { id: actingAgent(), model: null }, checkpoint: false, meta: { "agit.merge": { strategy: "merge", pull: n } } }, changes: detail.changes, patch: "", snapshot: srcHead.snapshot, signature: { status: "unsigned", trusted: false } };
        st.branches.set(targetBranch, [...tgt, ...extra, mergeCommit]);
        mergedId = mergeCommit.id;
      } else {
        st.branches.set(targetBranch, [...tgt, ...extra]);
        mergedId = srcHead.id;
      }
    } else {
      throw new ApiError(409, "merge_conflicts", "target moved since base; rebase the source branch", false);
    }
    pr.state = "merged";
    pr.merged_commit = mergedId;
    pr.updated_ms = Date.now();
    staleCache.clear();
    pushEvent(st, { kind: "ref.update", actor: actingAgent(), data: { ref: pr.target, old: pr.target_id, new: mergedId, op: "merge", pull: n } });
    pushEvent(st, { kind: "pull.merge", actor: actingAgent(), data: { number: n, commit: mergedId, strategy: body.strategy } });
    const draft = draftFromMerge(st, pr, mergedId, detail.changes, actingAgent());
    st.notes.unshift(draft);
    pushEvent(st, { kind: "note.draft", actor: actingAgent(), data: { id: draft.id, title: draft.title, pull: n, commit: mergedId, tags: draft.tags, paths: draft.paths } });
    return { pr: structuredClone(pr), merged_commit: mergedId };
  },
  async closePull(name, n) {
    await latency();
    const st = repoOf(name);
    const pr = st.pulls.find((p) => p.number === n) ?? notFound(`pull request #${n}`);
    if (pr.state !== "open") throw new ApiError(409, "invalid", `pull request #${n} is ${pr.state}`, false);
    pr.state = "closed";
    pr.updated_ms = Date.now();
    pushEvent(st, { kind: "pull.close", actor: actingAgent(), data: { number: n } });
    return structuredClone(pr);
  },
  async updatePull(name, n, body) {
    await latency();
    const st = repoOf(name);
    const pr = st.pulls.find((p) => p.number === n) ?? notFound(`pull request #${n}`);
    Object.assign(pr, Object.fromEntries(Object.entries(body).filter(([, v]) => v !== undefined)));
    pr.updated_ms = Date.now();
    return structuredClone(pr);
  },

  async notes(name, q = {}) {
    await latency();
    const st = repoOf(name);
    let list = st.notes.filter((n) => q.drafts || !n.draft);
    if (q.tags?.length) list = list.filter((n) => q.tags!.every((t) => n.tags.includes(t)));
    if (q.path) list = list.filter((n) => n.paths.some((p) => patternCovers(p, q.path!)));
    if (q.commit) {
      const id = resolve(st, q.commit).commit.id;
      list = list.filter((n) => n.commits.includes(id));
    }
    if (q.pull !== undefined) list = list.filter((n) => n.pulls.includes(q.pull!));
    if (q.q) {
      const scored = list.map((n) => ({ n, s: scoreNote(n, q.q!) })).filter((x) => x.s > 0).sort((a, b) => b.s - a.s);
      return scored.slice(0, q.limit ?? 100).map((x) => summary(st, x.n, x.s));
    }
    return [...list].sort((a, b) => Number(!!a.superseded_by) - Number(!!b.superseded_by) || b.updated_ms - a.updated_ms).slice(0, q.limit ?? 100).map((n) => summary(st, n));
  },
  async note(name, id) {
    await latency();
    const st = repoOf(name);
    const n = st.notes.find((x) => x.id === id) ?? notFound(`note ${id}`);
    return withStale(st, n);
  },
  async validateNote(name, id, rev) {
    await latency();
    const st = repoOf(name);
    require(name, "write");
    const n = st.notes.find((x) => x.id === id) ?? notFound(`note ${id}`);
    n.valid_at = resolve(st, rev ?? st.default_branch).commit.id;
    n.updated_ms = Date.now();
    pushEvent(st, { kind: "note.validate", actor: actingAgent(), data: { id: n.id, title: n.title, valid_at: n.valid_at } });
    return withStale(st, n);
  },
  async acceptNote(name, id, patch) {
    await latency();
    const st = repoOf(name);
    require(name, "write");
    const n = st.notes.find((x) => x.id === id) ?? notFound(`note ${id}`);
    if (patch) Object.assign(n, Object.fromEntries(Object.entries(patch).filter(([, v]) => v !== undefined)));
    n.draft = false;
    n.updated_ms = Date.now();
    pushEvent(st, { kind: "note.accept", actor: actingAgent(), data: { id: n.id, title: n.title, tags: n.tags } });
    return withStale(st, n);
  },
  async staleNotes(name, q = {}) {
    await latency();
    const st = repoOf(name);
    const out: T.StaleNote[] = [];
    for (const n of st.notes) {
      if (n.superseded_by || (n.draft && !q.drafts)) continue;
      const changed = changedPaths(st, n);
      if (changed.length) out.push({ ...summary(st, n), stale: true, changed_paths: changed });
    }
    return out.sort((a, b) => b.updated_ms - a.updated_ms);
  },
  async createNote(name, body) {
    await latency();
    const st = repoOf(name);
    if (!body.title?.trim()) throw new ApiError(400, "invalid", "title is required", false);
    const bad = body.tags.find((t) => !t.includes(":"));
    if (bad) throw new ApiError(400, "invalid", `tag "${bad}" has no namespace (expected namespace:value)`, false);
    require(name, "write");
    const now = Date.now();
    let valid_at: string | null = headOf(st, st.default_branch)?.id ?? null;
    if (body.valid_at) {
      if (!allCommits(st).has(body.valid_at)) throw new ApiError(412, "object_not_found", `valid_at ${body.valid_at} is not an object the server has`, false);
      valid_at = body.valid_at;
    }
    const n: T.Note = { id: "n_" + hashId(`note:${now}:${body.title}`).slice(0, 10), title: body.title, body: body.body, tags: [...new Set(body.tags)].sort(), paths: body.paths ?? [], commits: body.commits ?? [], pulls: body.pulls ?? [], author: actingAgent(), confidence: body.confidence ?? 0.8, created_ms: now, updated_ms: now, superseded_by: null, valid_at, draft: !!body.draft, stale: false };
    st.notes.unshift(n);
    pushEvent(st, n.draft ? { kind: "note.draft", actor: n.author, data: { id: n.id, title: n.title, tags: n.tags, paths: n.paths } } : { kind: "note.create", actor: n.author, data: { id: n.id, title: n.title, tags: n.tags } });
    return withStale(st, n);
  },
  async patchNote(name, id, body) {
    await latency();
    const st = repoOf(name);
    const n = st.notes.find((x) => x.id === id) ?? notFound(`note ${id}`);
    Object.assign(n, Object.fromEntries(Object.entries(body).filter(([, v]) => v !== undefined)));
    n.updated_ms = Date.now();
    pushEvent(st, { kind: "note.update", actor: actingAgent(), data: { id: n.id, title: n.title } });
    return withStale(st, n);
  },
  async deleteNote(name, id) {
    await latency();
    const st = repoOf(name);
    const idx = st.notes.findIndex((x) => x.id === id);
    if (idx < 0) notFound(`note ${id}`);
    const [n] = st.notes.splice(idx, 1);
    pushEvent(st, { kind: "note.delete", actor: actingAgent(), data: { id: n.id, title: n.title, draft: n.draft } });
    return { deleted: id };
  },
  async tagCounts(name) {
    await latency();
    const counts = new Map<string, number>();
    for (const n of repoOf(name).notes) if (!n.draft) for (const t of n.tags) counts.set(t, (counts.get(t) ?? 0) + 1);
    return [...counts.entries()].map(([tag, count]) => ({ tag, count })).sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag));
  },
  async rules(name) {
    await latency();
    return structuredClone(repoOf(name).rules);
  },
  async putRules(name, rules) {
    await latency();
    const st = repoOf(name);
    require(name, "admin");
    st.rules = structuredClone(rules);
    pushEvent(st, { kind: "knowledge.rules", actor: actingAgent(), data: { count: rules.length } });
    return structuredClone(rules);
  },
  async context(name, path, q = {}) {
    await latency();
    const st = repoOf(name);
    const limit = q.limit ?? 10;
    const now = Date.now();
    const notes = st.notes.filter((n) => !n.draft && n.paths.some((p) => patternCovers(p, path))).map((n) => summary(st, n)).sort((a, b) => Number(a.stale) - Number(b.stale)).slice(0, limit);
    const hist = [...resolve(st, st.default_branch).history].reverse();
    const touches = (c: BuiltCommit) => c.changes.some((ch) => patternCovers(path, ch.path) || patternCovers(ch.path, path));
    const recent_commits = hist.filter(touches).slice(0, limit).map(listItem);
    const leases = st.leases.filter((l) => (l.expires_ms ?? Infinity) > now && l.paths.some((p) => patternCovers(p, path) || patternCovers(path, p)));
    const agents_active = [...new Set([...leases.map((l) => l.holder), ...hist.filter((c) => touches(c) && now - c.commit.time_ms < DAY).map((c) => c.commit.agent?.id ?? c.commit.author.name)])].sort();
    let files: T.ContextFile[] | undefined;
    if (q.files) {
      const snap = headOf(st, st.default_branch)?.snapshot ?? new Map<string, string>();
      const isFile = snap.has(path);
      const dir = isFile ? (path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "") : path.replace(/\/$/, "");
      const lastTouched = new Map<string, number>();
      for (const c of hist) for (const ch of c.changes) if (!lastTouched.has(ch.path)) lastTouched.set(ch.path, c.commit.time_ms);
      const siblings = [...snap.keys()].filter((p) => p !== path && (p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : "") === dir).sort((a, b) => (lastTouched.get(b) ?? 0) - (lastTouched.get(a) ?? 0)).slice(0, 5);
      const candidates = (isFile ? [path, ...siblings] : siblings).filter((p) => !/[\u0000-\u0008]/.test(snap.get(p) ?? "") && !p.endsWith(".png"));
      files = candidates.map((p) => ({ path: p, text: snap.get(p) ?? "", truncated: false }));
      let budget = (q.budget ?? 6000) * 4 - 1500; // leave room for the rest of the response
      for (const f of files) budget -= f.text.length;
      for (let i = files.length - 1; i >= 0 && budget < 0; i--) {
        const f = files[i];
        if (f.text.length + budget > 200) {
          f.text = f.text.slice(0, f.text.length + budget);
          f.truncated = true;
          budget = 0;
        } else {
          budget += f.text.length;
          files.splice(i, 1);
        }
      }
    }
    const open_pulls: T.Pull[] = [];
    for (const pr of st.pulls.filter((p) => p.state === "open")) {
      const d = diffBetween(st, pr.base_id, pr.source_id);
      if (d.changes.some((c) => patternCovers(path, c.path) || patternCovers(c.path, path))) open_pulls.push(structuredClone(pr));
    }
    const suggested_tags = [...new Set(st.rules.filter((r) => patternCovers(r.pattern, path)).flatMap((r) => r.tags))].sort();
    const out: T.Context = { path, notes, suggested_tags, recent_commits, leases, open_pulls, agents_active };
    if (files) out.files = files;
    return out;
  },

  async policy(name) {
    await latency();
    return structuredClone(repoOf(name).policy);
  },
  async putPolicy(name, policy) {
    await latency();
    const st = repoOf(name);
    require(name, "admin");
    if (policy.require_approvals < 0 || policy.max_files_per_commit < 0) throw new ApiError(400, "invalid", "counts must be >= 0", false);
    st.policy = { ...T_DEFAULT_POLICY, ...structuredClone(policy) };
    pushEvent(st, { kind: "policy.update", actor: actingAgent(), data: { protected_branches: st.policy.protected_branches } });
    return structuredClone(st.policy);
  },

  async keys(name) {
    await latency();
    return structuredClone(repoOf(name).keys);
  },
  async addKey(name, body) {
    await latency();
    const st = repoOf(name);
    const who = require(name, "admin");
    if (!body.agent?.trim() || !body.public_key?.trim()) throw new ApiError(400, "invalid", "agent and public_key are required", false);
    const known = Object.entries(KEYS).find(([, k]) => k.public_key === body.public_key.trim());
    const key_id = known ? known[1].key_id : "k_" + hashId(`key:${body.public_key}`).slice(0, 16);
    const existing = st.keys.find((k) => k.key_id === key_id);
    if (existing && existing.agent !== body.agent.trim()) throw new ApiError(409, "already_exists", `key ${key_id} is registered for ${existing.agent}`, false);
    if (existing) return structuredClone(existing);
    const k: T.KeyEntry = { agent: body.agent.trim(), key_id, public_key: body.public_key.trim(), added_ms: Date.now(), added_by: who.agent ?? who.principal };
    st.keys.push(k);
    for (const list of st.branches.values()) for (const c of list) if (c.signature.status === "valid" && c.signature.key_id === k.key_id && c.signature.signer === k.agent) c.signature.trusted = true;
    pushEvent(st, { kind: "key.add", actor: actingAgent(), data: { agent: k.agent, key_id: k.key_id } });
    return structuredClone(k);
  },
  async deleteKey(name, keyId) {
    await latency();
    const st = repoOf(name);
    require(name, "admin");
    const idx = st.keys.findIndex((k) => k.key_id === keyId);
    if (idx < 0) notFound(`key ${keyId}`);
    const [k] = st.keys.splice(idx, 1);
    for (const list of st.branches.values()) for (const c of list) if (c.signature.status === "valid" && c.signature.key_id === k.key_id) c.signature.trusted = false;
    pushEvent(st, { kind: "key.delete", actor: actingAgent(), data: { agent: k.agent, key_id: k.key_id } });
    return { deleted: keyId };
  },

  async webhooks(name) {
    await latency();
    return structuredClone(repoOf(name).webhooks);
  },
  async createWebhook(name, body) {
    await latency();
    const st = repoOf(name);
    require(name, "admin");
    let url: URL;
    try {
      url = new URL(body.url);
    } catch {
      throw new ApiError(400, "invalid", "url must be absolute", false);
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") throw new ApiError(400, "invalid", "url must be http(s)", false);
    const wh: T.Webhook = { id: "wh_" + hashId(`wh:${Date.now()}:${body.url}`).slice(0, 8), url: body.url, kinds: [...new Set(body.kinds ?? [])], active: true, has_secret: !!body.secret, created_ms: Date.now() };
    st.webhooks.push(wh);
    pushEvent(st, { kind: "webhook.create", actor: actingAgent(), data: { id: wh.id, url: wh.url, kinds: wh.kinds } });
    return { ...wh };
  },
  async deleteWebhook(name, id) {
    await latency();
    const st = repoOf(name);
    require(name, "admin");
    const idx = st.webhooks.findIndex((w) => w.id === id);
    if (idx < 0) notFound(`webhook ${id}`);
    const [wh] = st.webhooks.splice(idx, 1);
    pushEvent(st, { kind: "webhook.delete", actor: actingAgent(), data: { id: wh.id, url: wh.url } });
    return { deleted: id };
  },
  async testWebhook(name, id) {
    await latency();
    const st = repoOf(name);
    if (!st.webhooks.some((w) => w.id === id)) notFound(`webhook ${id}`);
    return { queued: true };
  },

  async agents(name) {
    await latency();
    const st = repoOf(name);
    const now = Date.now();
    const m = new Map<string, T.AgentActivity>();
    const get = (id: string) => {
      let a = m.get(id);
      if (!a) m.set(id, (a = { id, commits: 0, last_seen_ms: 0, open_pulls: 0, leases: 0 }));
      return a;
    };
    for (const c of allCommits(st).values()) {
      if (!c.commit.agent) continue;
      const a = get(c.commit.agent.id);
      if (!a.model && c.commit.agent.model) a.model = c.commit.agent.model;
      if (!c.commit.checkpoint) a.commits++;
      a.last_seen_ms = Math.max(a.last_seen_ms, c.commit.time_ms);
    }
    for (const e of st.events) if (e.actor && m.has(e.actor)) get(e.actor).last_seen_ms = Math.max(get(e.actor).last_seen_ms, e.time_ms);
    for (const p of st.pulls) if (p.state === "open") get(p.author.id).open_pulls++;
    for (const l of st.leases) if ((l.expires_ms ?? Infinity) > now) get(l.holder).leases++;
    return [...m.values()].sort((a, b) => b.last_seen_ms - a.last_seen_ms);
  },
};
