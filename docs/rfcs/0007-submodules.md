# RFC 0007: Submodules and multi-repo

- Status: **Draft**
- Phase: format in 0 (✅), commands in 3, cross-repo PRs in 3
- Depends on: RFC 0001, RFC 0005

## Summary

A tree entry with mode `0o160000` (`submodule`) pins a commit id of a linked repository. `.agitmodules` (TOML) maps the path to a URL, branch and update policy. `agit submodule add | update | status | foreach` manage linked repositories; the server resolves pull requests that span several repositories so they merge atomically.

## Motivation

The agit monorepo itself is designed to split into component repositories (server, protocol, knowledge, git bridge, web, SDKs, spec) that are linked back into the main repository. The same need exists for every organization with shared libraries. git's submodules are notoriously awkward: detached HEADs, forgotten `--recursive`, no atomic cross-repo change. agit keeps the pinning model, because pinning is correct, and fixes the ergonomics and the cross-repo change problem.

## Design

### Tree encoding (✅ implemented)

```
mode = 0o160000, id = <commit id in the linked repository>, size = 0, name = <dir name>
```

The id is not expected to exist in the containing repository's object store. `fsck` does not treat a missing submodule commit as corruption. `status` and `ls-tree` report the entry as `submodule` with the pinned id.

### `.agitmodules`

TOML at the repository root, tracked like any file:

```toml
[submodule."web"]
path   = "web"
url    = "https://github.com/agit-vcs/agit-web"
branch = "main"
update = "checkout"   # checkout | rebase | merge | none
shallow = false
```

`update` controls what `agit submodule update` does when the pinned commit changes: `checkout` detaches at the pin (default, deterministic), `rebase`/`merge` integrate local work onto the pin, `none` leaves the working directory alone.

### Commands (📋)

- `agit submodule add <url> <path> [--branch b]`: clones into `<path>`, adds the `.agitmodules` entry, stages the submodule entry pinned at the clone's HEAD.
- `agit submodule update [--init] [--recursive] [path…]`: brings each submodule to its pinned commit per policy. `--init` clones missing ones. `--recursive` is the default when `.agitmodules` says so.
- `agit submodule status [--json]`: per submodule: path, pinned id, checked-out id, dirty flag, whether the pin is behind or ahead of `branch`.
- `agit submodule foreach <cmd>`: runs a command in each submodule with `AGIT_SUBMODULE_PATH` and `AGIT_SUBMODULE_ID` set.
- `agit commit` inside a parent with modified submodule HEADs updates the pins like any other change, and lease enforcement treats the submodule path as one path.

Submodule working directories store their `.agit/` under the parent's `.agit/modules/<path>/` with a pointer file at `<path>/.agit`, so removing a submodule directory does not lose its objects.

### Cross-repository pull requests (📋, server)

A **stacked PR set** is a group of PRs in several repositories plus one PR in the parent that updates the pins. The server:

1. validates that the parent PR's new pins equal the heads of the child PRs;
2. evaluates policies on each PR;
3. merges the children first, then the parent, in one coordinated sequence, and rolls back (closes without merging, restores refs by CAS) if any step fails;
4. records the whole sequence in each PR's decision log.

The guarantee is that the parent never points at a child commit that does not exist on the child's target branch.

### Identity and auth

Submodule URLs resolve through the same token scopes as any repository. Cloning a parent with `--recursive` requests tokens for each child; a child the principal cannot read is reported as `forbidden` for that path and the rest proceeds.

### Git bridge

Importing a git repository with submodules maps `160000` entries one to one and translates `.gitmodules` to `.agitmodules`. Submodule commit ids are translated through the SHA mapping table when the child repository has also been imported; otherwise the git id is kept in `meta` and the entry is marked unresolved.

## Alternatives

- **Vendoring (copying) instead of pinning.** Rejected: loses history and makes upstream changes a diff nightmare.
- **Monorepo only.** Rejected: organizations have boundaries, and the agit project itself wants component repositories.
- **Subtree merges.** Rejected: history becomes hard to reason about and cross-repo PRs are not expressible.
- **Submodule pins as refs rather than tree entries.** Rejected: pins must be part of the snapshot so a commit fully describes the tree.

## Open questions

- Whether `.agitmodules` should allow relative URLs (relative to the parent's remote).
- Default for `--recursive` on `clone`.
- Whether a stacked PR set should be a first-class object with its own id and API, or a label on member PRs.
- Lease semantics across repositories: whether a lease on `web/**` in the parent should imply anything in the `web` repository. Current position: no, leases are per repository.
- How `status` should report a submodule whose working directory has uncommitted changes versus one whose HEAD moved.
