// In-memory mock of the agit server API (docs/API.md) with realistic fixtures.
// Enabled with VITE_AGIT_MOCK=1 or automatically when GET /health fails.

import type { Api, StreamStatus } from "./client";
import { ApiError } from "./client";
import { loadSettings } from "./settings";
import type * as T from "./types";

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
}

interface BuiltCommit {
  id: string;
  commit: T.Commit;
  changes: T.Change[];
  patch: string;
  snapshot: Map<string, string>;
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
    const changes: T.Change[] = [];
    let patch = "";
    for (const [path, content] of Object.entries(st.changes)) {
      const before = snap.get(path) ?? null;
      if (content === null) next.delete(path);
      else next.set(path, content);
      const side = (c: string | null, p: string): T.ChangeSide | undefined => (c === null ? undefined : { id: hashId(`blob:${p}:${c}`), mode: "file", size: c.length });
      const kind: T.ChangeKind = before === null ? "added" : content === null ? "deleted" : "modified";
      changes.push({ path, kind, old: side(before, path), new: side(content, path) });
      patch += unifiedDiff(path, before, content);
    }
    changes.sort((a, b) => a.path.localeCompare(b.path));
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
    const id = hashId(`commit:${repo}:${parent}:${st.msg}:${st.at}`);
    out.push({ id, commit, changes, patch, snapshot: next });
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
  { msg: "Audit login outcomes", intent: "Make lockouts observable", task: "SHOP-40", agent: agents.coder1, at: at(3, 11), changes: { "src/auth/login.ts": F.login2 } },
  { msg: "Merge pull request #1 from ws/coder-2\n\nAudit invoices and refuse empty ones", intent: "Land billing hardening", task: "SHOP-41", agent: agents.reviewer, at: at(2, 16), changes: { "src/billing/invoice.ts": F.invoice2, "tests/billing.test.ts": F.billingTest }, meta: { "agit.merge": { strategy: "merge", pull: 1 } } },
  { msg: "Map AuthError to 401/423 and add POST /invoices", intent: "Expose billing over HTTP with proper auth status codes", task: "SHOP-42", agent: agents.coder1, at: at(1, 9, 30), changes: { "src/api/server.ts": F.server2 } },
  { msg: "Add logo asset", intent: "Branding for the storefront", task: "SHOP-50", at: at(1, 13), author: daniel, changes: { "assets/logo.png": "\u0089PNG\r\n\u001a\n\u0000\u0000\u0000\rIHDR\u0000\u0000" } },
  { msg: "Pin fastify to 5.3", intent: "Avoid the 5.4 schema regression", task: "SHOP-51", agent: agents.coder2, at: at(0, 8, 10), changes: { "package.json": F.pkg.replace('"^5.3.0"', '"5.3.0"') } },
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
  [{ msg: "Add Swiss VAT and jurisdiction listing", intent: "Prepare the CH launch", task: "SHOP-61", agent: agents.coder1, at: at(1, 17), changes: { "src/billing/tax.ts": F.taxConflict, "src/billing/invoice.ts": F.invoice1.replace('import { taxFor } from "./tax";', 'import { taxFor, knownJurisdictions } from "./tax";\n\nexport const JURISDICTIONS = knownJurisdictions();') } }],
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
  events: T.Event[];
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
  events: [],
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
  events: [],
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
  ...n,
});
shop.notes.push(
  note({ id: "n_auth_rotate", title: "Session tokens are rotated on password change", body: "Any code path that changes a password **must** call `SessionStore.rotate(userId)` afterwards. Forgetting this leaves old sessions valid.\n\nThe hook was added in SHOP-31. The in-memory store implements it; the Postgres store must do the same in one transaction with the password update.\n\n```ts\nawait store.setPassword(user.id, hash);\nawait store.rotate(user.id);\n```", tags: ["area:auth", "kind:gotcha", "lang:ts"], paths: ["src/auth/**"], commits: [mainAt(15).id], author: "coder-1", confidence: 0.95, created_ms: at(4, 9, 30), updated_ms: at(4, 9, 30) }),
  note({ id: "n_auth_lockout", title: "Decision: lock accounts after 5 failed logins for 15 minutes", body: "We chose a fixed lockout over exponential backoff because the edge rate limiter (SHOP-70) handles the volumetric case. Lockout state lives on the user record (`failedAttempts`), reset on success.\n\nAlternatives considered: CAPTCHA (rejected: no human users at this stage), IP bans (rejected: agents share egress).", tags: ["area:auth", "kind:decision"], paths: ["src/auth/login.ts"], commits: [mainAt(10).id], pulls: [], author: "planner-1", confidence: 0.9, created_ms: at(6, 11), updated_ms: at(6, 11) }),
  note({ id: "n_auth_errors", title: "How to add a new AuthError code", body: "1. Add the literal to `AuthCode` in `src/auth/errors.ts`.\n2. Map it to a status in `setErrorHandler` in `src/api/server.ts` (default is 401).\n3. Add a test in `tests/auth.test.ts` asserting `code`.\n\nDo not throw bare `Error` from anything under `src/auth`.", tags: ["area:auth", "area:api", "kind:howto"], paths: ["src/auth/errors.ts", "src/api/server.ts"], author: "coder-1", confidence: 0.85, created_ms: at(1, 10), updated_ms: at(1, 10) }),
  note({ id: "n_billing_cents", title: "All money is integer cents", body: "Never use floating point for amounts. `unitCents`, `subtotalCents`, `taxCents`, `totalCents` are integers; tax is `Math.round(subtotal * rate)` once per invoice, not per line, which is what the accountant signed off on.", tags: ["area:billing", "kind:rule", "service:billing"], paths: ["src/billing/**"], author: "coder-2", confidence: 1, created_ms: at(5, 9, 40), updated_ms: at(5, 9, 40) }),
  note({ id: "n_billing_tax_unknown", title: "Unknown jurisdictions must throw, not return 0", body: "Before PR #2 `taxFor` returned `0` for any unknown country. That silently under-charges. The new contract: throw. Callers validate country codes at the API boundary.\n\nSupersedes the earlier 'default to 0' decision.", tags: ["area:billing", "kind:decision", "service:billing"], paths: ["src/billing/tax.ts"], pulls: [2], author: "reviewer-1", confidence: 0.9, created_ms: at(0, 10, 45), updated_ms: at(0, 10, 45) }),
  note({ id: "n_billing_tax_default0", title: "Decision: unknown countries default to 0% tax", body: "Early decision, superseded: unknown countries were charged no tax to keep the demo working.", tags: ["area:billing", "kind:decision"], paths: ["src/billing/tax.ts"], author: "coder-2", confidence: 0.3, created_ms: at(7, 11, 45), updated_ms: at(0, 10, 45), superseded_by: "n_billing_tax_unknown" }),
  note({ id: "n_api_zod", title: "Validate every request body with zod at the route", body: "Routes parse their body with a zod schema before calling a domain function. Domain functions assume validated input and do not re-check shapes. Zod errors are returned as 400 by the default error handler.", tags: ["area:api", "kind:rule", "lang:ts"], paths: ["src/api/**"], author: "planner-1", confidence: 0.85 }),
  note({ id: "n_ci_fastify", title: "Gotcha: fastify 5.4 breaks schema compilation", body: "fastify 5.4.0 changed how `setErrorHandler` interacts with schema validation errors; our tests fail with `FST_ERR_VALIDATION`. Pinned to 5.3.0 in SHOP-51 until upstream fixes it (issue #5811).", tags: ["area:api", "kind:gotcha", "service:ci"], paths: ["package.json", "src/api/server.ts"], commits: [mainAt(23).id], author: "coder-2", confidence: 0.8, created_ms: at(0, 8, 15), updated_ms: at(0, 8, 15) }),
);
docs.notes.push(note({ id: "n_docs_palette", title: "Palette source of truth is the main README", body: "Accent colour must match the badge colour used in the root README. Currently `#1f6feb`.", tags: ["area:content", "kind:rule"], paths: ["styles.css"], author: "coder-2", confidence: 0.7 }));

// events: derive from history + leases + pulls, then sort by time
function seedEvents(st: RepoState) {
  const evs: Omit<T.Event, "seq">[] = [];
  evs.push({ time_ms: st.created_ms - 1000, kind: "repo.init", actor: "daniel", data: { name: st.name, default_branch: st.default_branch } });
  const seen = new Set<string>();
  for (const [branch, list] of st.branches) {
    for (const c of list) {
      if (seen.has(c.id)) continue;
      seen.add(c.id);
      const actor = c.commit.agent?.id ?? c.commit.author.name.toLowerCase();
      evs.push({ time_ms: c.commit.time_ms, kind: c.commit.checkpoint ? "checkpoint" : c.commit.parents.length > 1 || c.commit.message.startsWith("Merge") ? "merge" : "commit", workspace: branch.startsWith("ws/") ? branch.slice(3) : "main", actor, data: { id: c.id, branch, intent: c.commit.intent, task: c.commit.task } });
      evs.push({ time_ms: c.commit.time_ms + 5, kind: "ref.update", actor, data: { name: `refs/heads/${branch}`, old: c.commit.parents[0] ?? null, new: c.id } });
    }
  }
  for (const l of st.leases) evs.push({ time_ms: l.created_ms, kind: "lease.claim", workspace: l.workspace, actor: l.holder, data: { id: l.id, paths: l.paths, mode: l.mode, expires_ms: l.expires_ms } });
  for (const p of st.pulls) {
    evs.push({ time_ms: p.created_ms, kind: "pull.open", actor: p.author.id, data: { number: p.number, title: p.title, source: p.source, target: p.target } });
    for (const r of p.reviews) evs.push({ time_ms: r.time_ms, kind: "pull.review", actor: r.reviewer, data: { number: p.number, decision: r.decision } });
    if (p.state === "merged") evs.push({ time_ms: p.updated_ms, kind: "pull.merge", actor: "reviewer-1", data: { number: p.number, merged_commit: p.merged_commit, strategy: "merge" } });
    if (p.state === "closed") evs.push({ time_ms: p.updated_ms, kind: "pull.close", actor: "planner-1", data: { number: p.number } });
  }
  for (const n of st.notes) evs.push({ time_ms: n.created_ms, kind: "note.create", actor: n.author, data: { id: n.id, title: n.title, tags: n.tags } });
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
    } else if (roll === 2) pushEvent(st, { kind: "commit", workspace: a.id, actor: a.id, data: { id: hashId(`sim:${n}`), branch: `ws/${a.id}`, intent: intents[n % intents.length], task: `SHOP-${60 + (n % 12)}` } });
    else if (roll === 3) pushEvent(st, { kind: "ref.update", actor: a.id, data: { name: `refs/heads/ws/${a.id}`, old: hashId(`sim:${n - 1}`), new: hashId(`sim:${n}`) } });
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
  const changes: T.Change[] = [];
  let patch = "";
  const stats: T.FileStat[] = [];
  for (const p of paths) {
    const o = a.get(p) ?? null, n = b.get(p) ?? null;
    if (o === n) continue;
    const kind: T.ChangeKind = o === null ? "added" : n === null ? "deleted" : "modified";
    const side = (c: string | null): T.ChangeSide | undefined => (c === null ? undefined : { id: hashId(`blob:${p}:${c}`), mode: "file", size: c.length });
    changes.push({ path: p, kind, old: side(o), new: side(n) });
    const d = unifiedDiff(p, o, n);
    patch += d;
    let added = 0, removed = 0;
    for (const line of d.split("\n")) {
      if (line.startsWith("+") && !line.startsWith("+++")) added++;
      else if (line.startsWith("-") && !line.startsWith("---")) removed++;
    }
    stats.push({ path: p, added, removed });
  }
  return { changes, patch, stats };
}

function summary(n: T.Note): T.NoteSummary {
  const { body, ...rest } = n;
  const firstLine = body.split("\n").find((l) => l.trim() && !l.startsWith("#") && !l.startsWith("```")) ?? "";
  return { ...rest, excerpt: firstLine.slice(0, 160) };
}

function actingAgent() {
  return loadSettings().agent || "web-ui";
}

// ---- Api implementation ---------------------------------------------------------

export const mockApi: Api = {
  async health() {
    await sleep(20);
    return { status: "ok", version: "0.1.0-mock" };
  },

  async repos() {
    await latency();
    return [...repos.values()].map((st) => ({ name: st.name, default_branch: st.default_branch, head: headOf(st, st.default_branch)?.id ?? null, created_ms: st.created_ms }));
  },
  async createRepo(body) {
    await latency();
    if (repos.has(body.name)) throw new ApiError(409, "already_exists", `repository ${body.name} already exists`, false);
    const st: RepoState = { name: body.name, default_branch: body.default_branch ?? "main", created_ms: Date.now(), branches: new Map(), tags: [], leases: [], pulls: [], notes: [], rules: [], events: [] };
    repos.set(st.name, st);
    pushEvent(st, { kind: "repo.init", actor: actingAgent(), data: { name: st.name } });
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
    return { id: commit.id, commit: commit.commit, changes: commit.changes, patch: commit.patch };
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
    return { kind: "tree", path: p, entries: list };
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
    return st.events.filter((e) => e.seq > since).slice(0, limit);
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
        const mergeCommit: BuiltCommit = { id: hashId(`merge:${n}:${Date.now()}`), commit: { tree: srcHead.commit.tree, parents: [tgt[tgt.length - 1].id, srcHead.id], author: { name: actingAgent(), email: `${actingAgent()}@agents.local` }, time_ms: Date.now(), message: body.message ?? `Merge pull request #${n} from ${pr.source.replace("refs/heads/", "")}\n\n${pr.title}`, intent: pr.intent, task: pr.task, agent: { id: actingAgent(), model: null }, checkpoint: false, meta: { "agit.merge": { strategy: "merge", pull: n } } }, changes: detail.changes, patch: "", snapshot: srcHead.snapshot };
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
    pushEvent(st, { kind: "pull.merge", actor: actingAgent(), data: { number: n, merged_commit: mergedId, strategy: body.strategy } });
    pushEvent(st, { kind: "ref.update", actor: actingAgent(), data: { name: pr.target, new: mergedId } });
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
    let list = st.notes;
    if (q.tags?.length) list = list.filter((n) => q.tags!.every((t) => n.tags.includes(t)));
    if (q.path) list = list.filter((n) => n.paths.some((p) => patternCovers(p, q.path!)));
    if (q.q) {
      const needle = q.q.toLowerCase();
      list = list.filter((n) => n.title.toLowerCase().includes(needle) || n.body.toLowerCase().includes(needle) || n.tags.some((t) => t.includes(needle)));
    }
    return [...list].sort((a, b) => Number(!!a.superseded_by) - Number(!!b.superseded_by) || b.updated_ms - a.updated_ms).slice(0, q.limit ?? 50).map(summary);
  },
  async note(name, id) {
    await latency();
    const n = repoOf(name).notes.find((x) => x.id === id) ?? notFound(`note ${id}`);
    return structuredClone(n);
  },
  async createNote(name, body) {
    await latency();
    const st = repoOf(name);
    if (!body.title?.trim()) throw new ApiError(400, "invalid", "title is required", false);
    const bad = body.tags.find((t) => !t.includes(":"));
    if (bad) throw new ApiError(400, "invalid", `tag "${bad}" has no namespace (expected namespace:value)`, false);
    const now = Date.now();
    const n: T.Note = { id: "n_" + hashId(`note:${now}:${body.title}`).slice(0, 10), title: body.title, body: body.body, tags: [...new Set(body.tags)].sort(), paths: body.paths ?? [], commits: body.commits ?? [], pulls: body.pulls ?? [], author: actingAgent(), confidence: body.confidence ?? 0.8, created_ms: now, updated_ms: now, superseded_by: null };
    st.notes.unshift(n);
    pushEvent(st, { kind: "note.create", actor: n.author, data: { id: n.id, title: n.title, tags: n.tags } });
    return structuredClone(n);
  },
  async patchNote(name, id, body) {
    await latency();
    const st = repoOf(name);
    const n = st.notes.find((x) => x.id === id) ?? notFound(`note ${id}`);
    Object.assign(n, Object.fromEntries(Object.entries(body).filter(([, v]) => v !== undefined)));
    n.updated_ms = Date.now();
    return structuredClone(n);
  },
  async deleteNote(name, id) {
    await latency();
    const st = repoOf(name);
    const idx = st.notes.findIndex((x) => x.id === id);
    if (idx < 0) notFound(`note ${id}`);
    st.notes.splice(idx, 1);
    return { deleted: true };
  },
  async tagCounts(name) {
    await latency();
    const counts = new Map<string, number>();
    for (const n of repoOf(name).notes) for (const t of n.tags) counts.set(t, (counts.get(t) ?? 0) + 1);
    return [...counts.entries()].map(([tag, count]) => ({ tag, count })).sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag));
  },
  async rules(name) {
    await latency();
    return structuredClone(repoOf(name).rules);
  },
  async putRules(name, rules) {
    await latency();
    repoOf(name).rules = structuredClone(rules);
    return structuredClone(rules);
  },
  async context(name, path, limit = 10) {
    await latency();
    const st = repoOf(name);
    const now = Date.now();
    const notes = st.notes.filter((n) => n.paths.some((p) => patternCovers(p, path))).slice(0, limit).map(summary);
    const hist = [...resolve(st, st.default_branch).history].reverse();
    const recent_commits = hist.filter((c) => c.changes.some((ch) => patternCovers(path, ch.path) || patternCovers(ch.path, path))).slice(0, limit).map(listItem);
    const leases = st.leases.filter((l) => (l.expires_ms ?? Infinity) > now && l.paths.some((p) => patternCovers(p, path) || patternCovers(path, p)));
    const open_pulls: T.Pull[] = [];
    for (const pr of st.pulls.filter((p) => p.state === "open")) {
      const d = diffBetween(st, pr.base_id, pr.source_id);
      if (d.changes.some((c) => patternCovers(path, c.path) || patternCovers(c.path, path))) open_pulls.push(structuredClone(pr));
    }
    return { notes, recent_commits, leases, open_pulls };
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
      if (!c.commit.checkpoint) a.commits++;
      a.last_seen_ms = Math.max(a.last_seen_ms, c.commit.time_ms);
    }
    for (const e of st.events) if (e.actor && m.has(e.actor)) get(e.actor).last_seen_ms = Math.max(get(e.actor).last_seen_ms, e.time_ms);
    for (const p of st.pulls) if (p.state === "open") get(p.author.id).open_pulls++;
    for (const l of st.leases) if ((l.expires_ms ?? Infinity) > now) get(l.holder).leases++;
    return [...m.values()].sort((a, b) => b.last_seen_ms - a.last_seen_ms);
  },
};
