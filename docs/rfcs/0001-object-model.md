# RFC 0001: Object model

- Status: **Accepted**
- Phase: 0 (implemented in `crates/agit-core/src/{hash,object,store}.rs`)
- Supersedes: none

## Summary

agit stores repository content as immutable, content-addressed objects of three kinds (blob, tree, commit), identified by the BLAKE3-256 hash of a kind tag followed by the payload. Trees are a compact binary format carrying sizes; commits are canonical JSON with first-class fields for intent, task and agent identity. Loose objects are stored zstd-compressed under an `AGO1` header and verified on every read.

## Motivation

git's object model is the right foundation: content addressing gives integrity, deduplication and trivial distribution. Three things need to change for agents.

1. **Hash.** SHA-1 is collision-broken and slow. BLAKE3 is secure, several times faster per core, and parallel over large inputs.
2. **Metadata.** Agents need to record why a change was made, which task it serves, which agent and model made it, and arbitrary tool data, in a form that is queryable without parsing prose.
3. **Cheap answers.** `status`, `ls` and web views ask "how big is this file, what type is it" constantly. git must open the blob to know its size. Storing the size in the tree entry avoids that.

A fourth, smaller change: commits are JSON. Every agent already has a JSON parser; nobody should write a parser for a bespoke text format to read a commit.

## Design

### Identifiers

```
id = BLAKE3-256( kind_tag || payload )
kind_tag: blob = 0x01, tree = 0x02, commit = 0x03   (note = 0x04 reserved, RFC 0003)
```

Rendered as 64 lowercase hex characters. The kind tag is inside the hash so that no two kinds can share an id for the same bytes. `ObjectId::ZERO` is reserved. Prefix lookup needs at least 4 hex characters and is an error when ambiguous.

### Blob

Raw bytes. No normalization of any kind.

### Tree

```
"AGT1" | count:u32le | entry*
entry = mode:u32le | id:[u8;32] | size:u64le | name_len:u16le | name:[u8;name_len]
```

Modes: `0o100644` file, `0o100755` executable, `0o120000` symlink, `0o040000` dir, `0o160000` submodule. Entries sorted bytewise by name, unique names, names are UTF-8 without `/` or NUL. `size` is the blob length for files and symlinks, the entry count for directories, and `0` for submodules. Decoders reject anything that violates these rules.

### Commit

Canonical JSON, fields in this order, optional fields omitted when absent:

```json
{
  "tree": "<id>",
  "parents": ["<id>", "..."],
  "author": {"name": "", "email": ""},
  "time_ms": 0,
  "message": "",
  "intent": "optional",
  "task": "optional",
  "agent": {"id": "", "model": "optional", "run_id": "optional", "on_behalf_of": "optional"},
  "checkpoint": true,
  "meta": {"namespaced.key": "any json"}
}
```

`checkpoint` is omitted when false; `meta` when empty. `meta` keys should be namespaced (`tool.name`). Encoding is deterministic: struct field order plus `BTreeMap` for maps, so decode then encode reproduces the bytes and therefore the id.

### Loose object file

```
path   objects/<hex[0..2]>/<hex[2..64]>
bytes  "AGO1" | kind:u8 | raw_len:u64le | zstd(payload, level 3)
```

Write: temp file in the target directory, `fdatasync`, rename without clobbering. A lost rename race against an identical object is success. Read: decode, check `raw_len`, re-hash, compare; mismatch is an error and the bytes are not returned.

### Invariants

- Objects are immutable and never rewritten by a newer writer, because re-encoding could change the id.
- Unknown JSON fields are ignored on read and preserved nowhere; a writer that wants to carry extra data uses `meta`.
- Every read verifies. There is no "trusted" read path.

## Alternatives

- **SHA-256 for git compatibility.** Rejected: agit is not wire-compatible with git anyway (the git bridge maps ids), and BLAKE3's speed matters at the commit rates agit targets.
- **git's text tree format.** Rejected: no size field, and a parser that scans for NUL and space separators is slower and easier to get wrong than fixed-width fields.
- **Binary commits (bincode, CBOR).** Rejected for commits: readability by agents and humans with zero tooling outweighs the size difference; commits are small.
- **Separate committer field.** Rejected: `agent` and `meta` cover the rewrite-history case more precisely.
- **Storing the size in a separate cache.** Rejected: the tree is already the authoritative snapshot; a side cache adds a consistency problem.

## Open questions

- Whether `time_ms` should be accompanied by an explicit timezone offset for human display. Current position: UTC milliseconds only; display is a client concern.
- Whether `parents` should allow more than two entries (octopus merges). The format allows it; the merge command does not produce it yet.
- The exact `meta` namespace registry (`agit.*` reserved) is to be written in `spec/`.
