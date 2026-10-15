# agit format specification (planned)

Status: 📋 planned (Roadmap Phase 5, with drafts starting in Phase 2). This
directory holds only this README.

`spec/` will contain the normative, versioned specification of everything a
third-party implementation needs to interoperate with agit:

- Object model: object ids (BLAKE3-256 over kind tag and payload), blob, tree
  (`AGT1`), commit (canonical JSON), and later `note`.
- Loose object file format (`AGO1`), pack segment format, journal format.
- Repository layout under `.agit/`, index (`AGI1`), refs, leases, events.
- JSON schemas for every `--json` output and for every error, each with a
  `version` field.
- Wire protocol: messages, pack streaming, authentication, idempotency.

Until the 1.0 format freeze, the informal description in
`docs/ARCHITECTURE.md` and the RFCs in `docs/rfcs/` are the reference, and
the Rust code in `crates/agit-core` is the implementation of record.

This component is designed to be split into its own repository and linked
back as a submodule (`.agitmodules`).
