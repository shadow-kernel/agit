# Security model

This document describes what agit protects today, what it does not, how to
harden a deployment, and how to report vulnerabilities. It reflects the
single-node server in `crates/agit-server` as it stands; the planned
multi-tenant design (organisations, per-agent scoped tokens, RFC 0004) is
not built and is not covered here. Deployment mechanics are in
[DEPLOYMENT.md](DEPLOYMENT.md).

## Summary

- Authentication is **one shared bearer token** per server
  (`AGIT_SERVER_TOKEN`). It grants full read/write access to every repository
  and setting. No token means an open server.
- The acting agent (`X-Agit-Agent`, `AGIT_AGENT`) is **self-asserted**. It is
  recorded in commits, events, leases, PRs and reviews and used by policies,
  but nothing verifies that the caller is that agent.
- **Webhook deliveries are signed** with keyed BLAKE3 when a secret is set;
  receivers should verify the signature.
- **Objects are verified on read**: every blob, tree and commit is re-hashed
  against its BLAKE3 id when loaded, so corrupted or tampered storage is
  detected rather than served.
- **No TLS in the server.** Terminate TLS in a reverse proxy.
- **No encryption at rest, no rate limiting, no request size limits** in the
  server itself. Provide them at the filesystem, proxy and network layers.

## Assets

- Repository contents (objects, refs) and their history, including commit
  metadata: intent, task, agent, model, run id and arbitrary `--meta`.
- Pull requests, reviews and comments; policies; leases; the event log.
- Knowledge notes, which agents use as working memory and which may contain
  decisions, credentials pasted by mistake, or internal URLs.
- Webhook subscriptions with their secrets (`webhooks.json`, plaintext on
  disk) and the trusted signing-key registry (`keys.json`).
- The bearer token itself, held by every client, SDK configuration, CI job
  and browser that uses the web UI.

## Trust boundaries and what holds at each

### Network client to server

Every request under `/api/v1` passes through one middleware: if the server has
a token, the request must carry `Authorization: Bearer <token>` exactly;
otherwise it is rejected with `403`. That is the whole access model:

- There are no roles, no per-repository permissions and no read-only tokens.
  Anyone with the token can create and delete repositories, move refs
  (subject to policy), change policies, register webhooks pointing at any
  URL, and add trusted signing keys.
- Token comparison is a plain string comparison; tokens should be long and
  random (`openssl rand -hex 32`). An empty `AGIT_SERVER_TOKEN=` is treated
  as an empty token, not as "disabled".
- `/api/v1/health` is behind the same check, so an unauthenticated probe
  learns nothing but "something answers with 403".
- Policies (`protected branches`, `allowed merge/push agents`,
  `require_agent`, minimum approvals, review decisions) are enforced, but
  they key on the self-asserted `X-Agit-Agent`. They defend against
  well-behaved agents making mistakes, not against a caller who lies about
  who they are. The same applies to "author excluded from approvals" and to
  lease ownership.
- Static files under `AGIT_WEB_DIR` are served **without** the token check
  (only `/api/v1` is protected). The UI bundle is public by design; it holds
  no secrets.

Landing on `main` now: Ed25519 commit signing (`agit key generate`,
`agit commit --sign`) with a per-repository registry of trusted public keys
per agent. A valid, trusted signature does prove that whoever made the commit
held the agent's private key, which is the first real agent identity in the
system. Note that today anyone with the bearer token may add keys to the
registry, so key trust is only as strong as token custody.

### Server to webhook receivers

Each delivery is a `POST` with headers `X-Agit-Event`, `X-Agit-Repo`,
`X-Agit-Delivery` (unique id) and `X-Agit-Webhook`. When the subscription has
a secret, `X-Agit-Signature: blake3=<hex>` is
`blake3::keyed_hash(derive_key("agit webhook v1", secret), body)` over the raw
body. Receivers should recompute it with a constant-time compare and treat
`X-Agit-Delivery` as an idempotency key (deliveries retry three times with
backoff, so duplicates are possible).

The server performs outbound HTTP to whatever URL a token holder registers.
That is a server-side request forgery (SSRF) surface: a token holder can make
the server POST to addresses on its own network (cloud metadata endpoints,
internal services). The body is an agit event, not attacker-chosen bytes, and
responses are not returned to the caller, but the request still happens.
Restrict the server's egress with a network policy or firewall if it runs in
an environment where that matters.

### Server to disk

- Objects are content-addressed (BLAKE3) and verified when read, so a
  flipped bit, a truncated file or a swapped object is reported as
  corruption. `agit fsck` checks a repository end to end.
- Writes are atomic (temp file + rename; refs fsynced), so a crash leaves no
  torn objects or refs. See `AGIT_FSYNC` in DEPLOYMENT.md.
- Nothing is encrypted at rest. Webhook secrets are stored in plaintext. Use
  disk encryption and restrict the data directory to the service user
  (`0750`, as the systemd unit does).
- Repository names come from the URL path and are used as directory names;
  the server validates them, but keep the data directory dedicated to agit
  and never point `AGIT_DATA_DIR` at a directory that holds other things.

### Web UI

- The UI keeps the bearer token, the API base URL and the agent name in the
  browser's `localStorage`. Anyone who can run script on the UI's origin
  (an XSS, or a browser extension) can read it. Markdown is rendered through
  DOMPurify, and the UI uses no cookies, so there is no CSRF channel.
- The server answers `Access-Control-Allow-Origin: *`. Since the credential
  is a header that the browser never attaches automatically, this does not
  let another website use a logged-in session; it does mean any site can
  make unauthenticated requests to the server, which only matters when the
  server runs without a token.

### CLI and local repositories

- `agit` operates on local files under `.agit/` with advisory `flock`
  locks. It trusts the working tree and `.agit` as the user's own data.
- `agit git import|export` executes the `git` binary found on `PATH` with
  the given repository path. Run it on repositories you trust, as you would
  `git`.
- Agent metadata on commits (`--agent`, `--model`, `--meta`) is free-form and
  user-supplied; do not treat it as evidence of who ran the command.

## Out of scope today

Denial of service (no rate limits, no body size limits, no per-token
quotas), multi-tenancy, audit logging beyond the per-repository event log,
secrets scanning of pushed content, and any protection against a token
holder. Each of these is addressed at the proxy or network layer for now and
is tracked for the server in [ROADMAP.md](ROADMAP.md) and the RFCs.

## Hardening checklist

Server:

- [ ] Set `AGIT_SERVER_TOKEN` to at least 32 random bytes; never run an
      open server on a reachable interface.
- [ ] Bind to `127.0.0.1` (or a private interface) and put a TLS-terminating
      reverse proxy in front; enable HSTS once TLS works.
- [ ] Run as a dedicated unprivileged user (the systemd unit) or the image's
      uid 10001; data directory mode `0750`.
- [ ] Use the shipped systemd hardening (`ProtectSystem=strict`,
      `NoNewPrivileges`, empty capability set, syscall filter) or equivalent
      container settings (read-only root filesystem, `no-new-privileges`,
      dropped capabilities; the image needs write access only to `/data`).
- [ ] Limit request body size and connection rates at the proxy for servers
      reachable by untrusted networks.
- [ ] Restrict outbound traffic from the server host to the webhook
      receivers you expect.
- [ ] Encrypt the disk or volume holding `AGIT_DATA_DIR`; encrypt backups
      (they contain webhook secrets and all content).
- [ ] Keep one server per trust domain; do not share a token between teams
      or environments.

Clients and agents:

- [ ] Give agents the token through the environment (`AGIT_TOKEN`) or a
      secrets manager, not on the command line or in prompts; scrub it from
      logs and transcripts.
- [ ] Rotate the token when a client is decommissioned or a transcript
      containing it leaks (restart the server with a new value).
- [ ] Verify webhook signatures and use `X-Agit-Delivery` for idempotency.
- [ ] Configure policies (protected branches, PR-only, required approvals,
      `require_intent`) to catch agent mistakes, and remember they are not
      an authentication boundary.
- [ ] Pin SDK and CLI versions in agent environments; the installer supports
      `--version` and verifies checksums.

Operations:

- [ ] Back up the data directory (DEPLOYMENT.md) and test restores.
- [ ] Watch the event stream or webhook deliveries for unexpected ref
      updates, policy changes and repository deletions.
- [ ] Follow releases; pre-1.0 security fixes ship only in the latest
      version.

## Responsible disclosure

Report vulnerabilities privately through GitHub's vulnerability reporting
for this repository:
<https://github.com/shadow-kernel/agit/security/advisories/new>. If that page is
unavailable, contact a maintainer listed in the repository directly and ask
for a private channel. Please do not file public issues for security
problems.

What to include: the version or commit, a description of the issue and its
impact, and reproduction steps or a proof of concept. Encrypting the report
is welcome but not required.

What to expect: acknowledgement within 72 hours, an assessment within
7 days, and a fix or mitigation in the next release, with credit in the
release notes unless you prefer otherwise. We ask for 90 days before public
disclosure, shorter by agreement once a fix is available.

Supported versions: agit is pre-1.0. Fixes land on `main` and in the latest
release only.
